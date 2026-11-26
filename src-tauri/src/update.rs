use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tauri::{ipc::Channel, AppHandle, Manager};
use tokio::io::AsyncWriteExt;

const RELEASES_API: &str = "/repos/tttnny/dsh-launcher/releases?per_page=100";
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;
const SIGNED_INSTALLER_LABEL: &str = "Signed macOS installer";
#[derive(Clone)]
struct DownloadedUpdate {
    path: std::path::PathBuf,
    version: semver::Version,
    url: String,
    published_at: Option<String>,
    size: u64,
    hash: String,
}

static DOWNLOADED_UPDATE: tokio::sync::Mutex<Option<DownloadedUpdate>> =
    tokio::sync::Mutex::const_new(None);

#[derive(Clone, Debug, Serialize)]
pub struct LauncherUpdateInfo {
    pub current: String,
    pub channel: String,
    pub up_to_date: bool,
    pub latest: Option<String>,
    pub url: Option<String>,
    pub published_at: Option<String>,
    pub has_installer: bool,
    pub ready_to_restart: bool,
}

#[derive(Debug, Deserialize)]
struct ReleaseEntry {
    tag_name: String,
    html_url: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    size: u64,
    digest: Option<String>,
    label: Option<String>,
}

fn mac_installer<'a>(release: &'a ReleaseEntry, arch: &str) -> Option<(&'a ReleaseAsset, &'a str)> {
    let version = parse_tag(&release.tag_name)?;
    ["universal", arch].into_iter().find_map(|arch| {
        let name = format!("dsh-launcher_{version}_{arch}.dmg");
        let asset = release.assets.iter().find(|asset| {
            asset.name == name && asset.label.as_deref() == Some(SIGNED_INSTALLER_LABEL)
        })?;
        let hash = asset.digest.as_deref()?.strip_prefix("sha256:")?;
        (hash.len() == 64
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && (1..=MAX_INSTALLER_BYTES).contains(&asset.size))
        .then_some((asset, hash))
    })
}

fn parse_tag(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

fn parse_releases_json(body: &[u8]) -> Result<Vec<ReleaseEntry>, String> {
    serde_json::from_slice(body).map_err(|e| format!("解析 GitHub Releases JSON 失败: {e}"))
}

fn pick_latest<'a>(
    current: &semver::Version,
    dev_channel: bool,
    releases: &'a [ReleaseEntry],
) -> Option<&'a ReleaseEntry> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| parse_tag(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| dev_channel || v.pre.is_empty())
        .filter(|(v, _)| v > current)
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, r)| r)
}

fn parse_channel(raw: Option<&str>, current: &semver::Version) -> Result<bool, String> {
    match raw
        .map(|channel| channel.trim().to_ascii_lowercase())
        .as_deref()
    {
        None => Ok(!current.pre.is_empty()),
        Some("dev") => Ok(true),
        Some("release") => Ok(false),
        Some(other) => Err(format!("无效的更新渠道: {other}")),
    }
}

fn current_version() -> Result<semver::Version, String> {
    let version = env!("CARGO_PKG_VERSION");
    semver::Version::parse(version).map_err(|e| format!("当前版本号无效 {version}: {e}"))
}

fn http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    crate::proxy::apply(reqwest::Client::builder())
        .timeout(timeout)
        .user_agent("dsh-launcher")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

async fn fetch_releases() -> Result<Vec<ReleaseEntry>, String> {
    let url = crate::plugins::github_api_url(RELEASES_API);
    let client = http_client(Duration::from_secs(15))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("请求 GitHub Releases 失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub Releases 返回 HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err("GitHub Releases 响应过大".to_string());
    }
    parse_releases_json(&bytes)
}

#[tauri::command]
pub async fn check_launcher_update(channel: Option<String>) -> Result<LauncherUpdateInfo, String> {
    let current = current_version()?;
    let current_raw = env!("CARGO_PKG_VERSION");
    let dev_channel = parse_channel(channel.as_deref(), &current)?;
    let channel = if dev_channel { "dev" } else { "stable" }.to_string();
    if let Some(update) = DOWNLOADED_UPDATE.lock().await.as_ref() {
        if dev_channel || update.version.pre.is_empty() {
            return Ok(LauncherUpdateInfo {
                current: current_raw.to_string(),
                channel,
                up_to_date: false,
                latest: Some(update.version.to_string()),
                url: Some(update.url.clone()),
                published_at: update.published_at.clone(),
                has_installer: true,
                ready_to_restart: true,
            });
        }
    }
    let releases = fetch_releases().await?;
    crate::log_debug!("GitHub Releases API 解析出 {} 条发布记录", releases.len());

    let latest = pick_latest(&current, dev_channel, &releases);
    match latest {
        Some(r) => crate::log_info!(
            "发现新版本 {}（当前 {current_raw}，{dev} 渠道）",
            r.tag_name,
            dev = if dev_channel { "dev" } else { "release" }
        ),
        None => crate::log_info!("启动器已是最新（{current_raw}）"),
    }
    Ok(LauncherUpdateInfo {
        current: current_raw.to_string(),
        channel,
        up_to_date: latest.is_none(),
        latest: latest.map(|r| r.tag_name.trim_start_matches('v').to_string()),
        url: latest.map(|r| r.html_url.clone()),
        published_at: latest.and_then(|r| r.published_at.clone()),
        has_installer: latest
            .and_then(|r| mac_installer(r, std::env::consts::ARCH))
            .is_some()
            && can_update_in_place().await,
        ready_to_restart: false,
    })
}

#[tauri::command]
pub async fn download_launcher_update(
    app: AppHandle,
    version: String,
    on_progress: Channel<u8>,
) -> Result<(), String> {
    let mut downloaded = DOWNLOADED_UPDATE
        .try_lock()
        .map_err(|_| "启动器正在下载或安装更新".to_string())?;
    if !can_update_in_place().await {
        return Err("当前启动器无法原位更新，请打开发布页面手动安装签名版".into());
    }
    let current = current_version()?;
    let version = semver::Version::parse(&version).map_err(|e| format!("无效的版本号: {e}"))?;
    if version <= current || (current.pre.is_empty() && !version.pre.is_empty()) {
        return Err("该版本不适用于当前启动器".into());
    }
    let releases = fetch_releases().await?;
    let release = releases
        .iter()
        .find(|r| !r.draft && parse_tag(&r.tag_name).as_ref() == Some(&version))
        .ok_or("找不到该版本，请重新检查更新")?;
    let (asset, hash) = mac_installer(release, std::env::consts::ARCH)
        .ok_or("该版本没有适用于本机且可校验的安装包")?;
    let url = format!(
        "https://github.com/tttnny/dsh-launcher/releases/download/{}/{}",
        release.tag_name, asset.name
    );
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("launcher-update");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("创建下载目录失败: {e}"))?;
    let path = dir.join(&asset.name);
    let partial = dir.join(format!("{}.part", uuid::Uuid::new_v4()));
    let result = download_installer(&url, asset.size, hash, &partial, &on_progress).await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(error);
    }
    if let Err(error) = tokio::fs::rename(&partial, &path).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(format!("保存安装包失败: {error}"));
    }
    if let Some(previous) = downloaded.replace(DownloadedUpdate {
        path: path.clone(),
        version,
        url: release.html_url.clone(),
        published_at: release.published_at.clone(),
        size: asset.size,
        hash: hash.to_string(),
    }) {
        if previous.path != path {
            let _ = tokio::fs::remove_file(previous.path).await;
        }
    }
    Ok(())
}

async fn can_update_in_place() -> bool {
    #[cfg(target_os = "macos")]
    {
        tauri::async_runtime::spawn_blocking(macos::installed_identity)
            .await
            .is_ok_and(|result| result.is_ok())
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RestartAction {
    Retry,
    Download,
    Manual,
}

#[derive(Debug, Serialize)]
pub struct RestartError {
    message: String,
    action: RestartAction,
}

#[cfg(target_os = "macos")]
impl RestartError {
    fn manual(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            action: RestartAction::Manual,
        }
    }
}

impl From<String> for RestartError {
    fn from(message: String) -> Self {
        Self {
            message,
            action: RestartAction::Retry,
        }
    }
}

static UPDATE_RELAUNCH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub fn relaunch_after_exit() {
    if let Some(executable) = UPDATE_RELAUNCH.get() {
        if let Err(error) = std::process::Command::new(executable).spawn() {
            crate::log_warn!("更新安装完成，重新打开启动器失败：{error}");
        }
    }
}

#[tauri::command]
pub async fn restart_launcher_update(app: AppHandle) -> Result<(), RestartError> {
    let mut downloaded = DOWNLOADED_UPDATE
        .try_lock()
        .map_err(|_| "启动器正在下载或安装更新".to_string())?;
    let update = downloaded.as_ref().ok_or_else(|| RestartError {
        message: "请先下载启动器更新".into(),
        action: RestartAction::Download,
    })?;
    #[cfg(target_os = "macos")]
    {
        let install = update.clone();
        let result = tauri::async_runtime::spawn_blocking(move || macos::install(&install))
            .await
            .map_err(|e| format!("安装更新失败：{e}"))?;
        let executable = match result {
            Ok(executable) => executable,
            Err(error) => {
                if !matches!(error.action, RestartAction::Retry) {
                    let _ = tokio::fs::remove_file(&update.path).await;
                    *downloaded = None;
                }
                return Err(error);
            }
        };
        let _ = UPDATE_RELAUNCH.set(executable);
        let _ = tokio::fs::remove_file(&update.path).await;
        *downloaded = None;
        app.exit(0);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, update, &mut downloaded);
        Err("当前平台无法原位更新".to_string().into())
    }
}

async fn download_installer(
    url: &str,
    size: u64,
    hash: &str,
    path: &Path,
    progress: &Channel<u8>,
) -> Result<(), String> {
    let client = http_client(Duration::from_secs(600))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("请求安装包失败: {e}"))?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|e| format!("创建下载文件失败: {e}"))?;
    let mut received = 0;
    let mut last_percent = 0;
    let mut digest = Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下载中断: {e}"))?
    {
        received += chunk.len() as u64;
        if received > size {
            return Err("安装包大小与 GitHub 发布信息不符".into());
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入安装包失败: {e}"))?;
        digest.update(&chunk);
        let percent = (received * 100 / size).min(99) as u8;
        if percent > last_percent {
            let _ = progress.send(percent);
            last_percent = percent;
        }
    }
    file.flush()
        .await
        .map_err(|e| format!("写入安装包失败: {e}"))?;
    if received != size || !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(hash) {
        return Err("安装包校验失败，请重新下载".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{DownloadedUpdate, RestartAction, RestartError};
    use sha2::{Digest, Sha256};
    use std::ffi::{CString, OsStr};
    use std::fs::{self, DirBuilder, File};
    use std::io::{self, Read};
    use std::os::unix::{ffi::OsStrExt, fs::DirBuilderExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    const BUNDLE_ID: &str = "in.dsh-plug.dsh-launcher";
    const BUNDLE_NAME: &str = "dsh-launcher.app";

    struct StagingDir(PathBuf);

    impl Drop for StagingDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.join(BUNDLE_NAME));
            let _ = fs::remove_file(self.0.join("update.dmg"));
            let _ = fs::remove_dir(self.0.join("image"));
            let _ = fs::remove_dir(&self.0);
        }
    }

    struct MountedImage(PathBuf);

    impl Drop for MountedImage {
        fn drop(&mut self) {
            if let Err(error) = self.detach() {
                crate::log_warn!("清理启动器更新磁盘映像失败：{error}");
            }
        }
    }

    impl MountedImage {
        fn detach(&self) -> Result<(), String> {
            let mut command = Command::new("/usr/bin/hdiutil");
            command.arg("detach").arg(&self.0);
            run(&mut command, "卸载更新磁盘映像").map(|_| ())
        }
    }

    fn run(command: &mut Command, action: &str) -> Result<Output, String> {
        let output = command.output().map_err(|e| format!("{action}失败：{e}"))?;
        if output.status.success() {
            Ok(output)
        } else {
            let detail = String::from_utf8_lossy(&output.stderr);
            Err(format!("{action}失败：{}", detail.trim()))
        }
    }

    fn plist_value(bundle: &Path, key: &str) -> Result<String, String> {
        let output = run(
            Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", &format!("Print :{key}")])
                .arg(bundle.join("Contents/Info.plist")),
            "读取应用信息",
        )?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn signed_requirement(bundle: &Path) -> Result<String, String> {
        run(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict", "--all-architectures"])
                .arg(bundle),
            "验证应用签名",
        )?;
        let output = run(
            Command::new("/usr/bin/codesign")
                .args(["-d", "-r-"])
                .arg(bundle),
            "读取应用签名身份",
        )?;
        let requirement = [&output.stdout, &output.stderr].iter().find_map(|stream| {
            String::from_utf8_lossy(stream)
                .lines()
                .find_map(|line| line.strip_prefix("designated => ").map(str::to_string))
        });
        requirement
            .filter(|requirement| {
                requirement.contains("anchor apple") && !requirement.contains("cdhash")
            })
            .ok_or_else(|| "应用缺少可跨版本保留授权的签名身份".to_string())
    }

    pub(super) fn installed_identity() -> Result<(PathBuf, String), String> {
        let exe = tauri::process::current_binary(&tauri::Env::default())
            .map_err(|e| format!("找不到当前启动器：{e}"))?;
        let bundle = exe
            .ancestors()
            .nth(3)
            .filter(|bundle| bundle.extension() == Some(OsStr::new("app")))
            .filter(|bundle| exe == bundle.join("Contents/MacOS/dsh-launcher"))
            .ok_or("请从已安装的启动器应用检查更新")?;
        if plist_value(bundle, "CFBundleIdentifier")? != BUNDLE_ID {
            return Err("当前应用标识无效".into());
        }
        let parent = bundle.parent().ok_or("找不到应用安装目录")?;
        let parent = CString::new(parent.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        if unsafe { libc::access(parent.as_ptr(), libc::W_OK) } != 0 {
            return Err("应用安装目录不可写".into());
        }
        Ok((bundle.to_path_buf(), signed_requirement(bundle)?))
    }

    fn copy_verified_image(
        update: &DownloadedUpdate,
        destination: &Path,
    ) -> Result<(), RestartError> {
        let image = File::open(&update.path).map_err(|error| RestartError {
            message: format!("读取安装包失败：{error}"),
            action: if error.kind() == io::ErrorKind::NotFound {
                RestartAction::Download
            } else {
                RestartAction::Retry
            },
        })?;
        let mut staged =
            File::create_new(destination).map_err(|e| format!("准备安装包失败：{e}"))?;
        let size = io::copy(&mut image.take(update.size + 1), &mut staged)
            .map_err(|e| format!("复制安装包失败：{e}"))?;
        drop(staged);
        let mut digest = Sha256::new();
        let mut staged = File::open(destination).map_err(|e| format!("读取安装包失败：{e}"))?;
        io::copy(&mut staged, &mut digest).map_err(|e| format!("校验安装包失败：{e}"))?;
        if size != update.size
            || !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(&update.hash)
        {
            return Err(RestartError {
                message: "安装包校验失败，请重新下载".into(),
                action: RestartAction::Download,
            });
        }
        Ok(())
    }

    fn swap_bundles(installed: &Path, staged: &Path) -> io::Result<()> {
        let installed = CString::new(installed.as_os_str().as_bytes())?;
        let staged = CString::new(staged.as_os_str().as_bytes())?;
        let result =
            unsafe { libc::renamex_np(installed.as_ptr(), staged.as_ptr(), libc::RENAME_SWAP) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn install(update: &DownloadedUpdate) -> Result<PathBuf, RestartError> {
        let (installed, requirement) = installed_identity()?;

        let parent = installed
            .parent()
            .ok_or_else(|| "找不到应用安装目录".to_string())?;
        let staging_path = parent.join(format!(".dsh-launcher-update-{}", uuid::Uuid::new_v4()));
        DirBuilder::new()
            .mode(0o700)
            .create(&staging_path)
            .map_err(|e| format!("无法在应用安装目录准备更新：{e}"))?;
        let staging = StagingDir(staging_path);
        let image = staging.0.join("update.dmg");
        copy_verified_image(update, &image)?;
        let mount = staging.0.join("image");
        fs::create_dir(&mount).map_err(|e| format!("准备更新磁盘映像失败：{e}"))?;
        let staged = staging.0.join(BUNDLE_NAME);
        {
            run(
                Command::new("/usr/bin/hdiutil")
                    .args([
                        "attach",
                        "-readonly",
                        "-nobrowse",
                        "-noautoopen",
                        "-mountpoint",
                    ])
                    .arg(&mount)
                    .arg(&image),
                "挂载更新磁盘映像",
            )?;
            let mounted = MountedImage(mount);
            let source = mounted.0.join(BUNDLE_NAME);
            if !fs::symlink_metadata(&source).is_ok_and(|info| info.is_dir()) {
                return Err(RestartError::manual(
                    "安装包缺少启动器应用，请在发布页面查看安装说明",
                ));
            }
            run(
                Command::new("/usr/bin/ditto").arg(source).arg(&staged),
                "复制新版本",
            )?;
        }
        if plist_value(&staged, "CFBundleIdentifier").map_err(RestartError::manual)? != BUNDLE_ID
            || plist_value(&staged, "CFBundleExecutable").map_err(RestartError::manual)?
                != "dsh-launcher"
            || plist_value(&staged, "CFBundleShortVersionString").map_err(RestartError::manual)?
                != update.version.to_string()
        {
            return Err(RestartError::manual(
                "安装包中的应用标识、入口或版本与发布信息不符",
            ));
        }
        if signed_requirement(&staged).map_err(RestartError::manual)? != requirement {
            return Err(RestartError::manual(
                "新版应用签名身份与当前版本不同，请在发布页面查看安装说明",
            ));
        }
        run(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--all-architectures"])
                .arg(format!("-R={requirement}"))
                .arg(&staged),
            "验证新版本的签名身份",
        )
        .map_err(RestartError::manual)?;
        swap_bundles(&installed, &staged).map_err(|e| format!("替换启动器失败：{e}"))?;
        Ok(installed.join("Contents/MacOS/dsh-launcher"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> ReleaseEntry {
        ReleaseEntry {
            tag_name: tag.to_string(),
            html_url: format!("https://example.com/releases/tag/{tag}"),
            published_at: None,
            draft: false,
            assets: Vec::new(),
        }
    }

    fn tag_of(found: Option<&ReleaseEntry>) -> Option<String> {
        found.map(|r| r.tag_name.clone())
    }

    #[tokio::test]
    async fn checking_returns_downloaded_update_without_fetching_releases() {
        let version = semver::Version::parse("9999.0.0").unwrap();
        let url = "https://example.com/releases/tag/v9999.0.0".to_string();
        *DOWNLOADED_UPDATE.lock().await = Some(DownloadedUpdate {
            path: "fixture.dmg".into(),
            version: version.clone(),
            url: url.clone(),
            published_at: None,
            size: 1,
            hash: String::new(),
        });
        let result = check_launcher_update(None).await;
        *DOWNLOADED_UPDATE.lock().await = None;
        let info = result.unwrap();
        assert!(info.ready_to_restart);
        assert_eq!(info.latest, Some(version.to_string()));
        assert_eq!(info.url, Some(url));
    }

    #[test]
    fn json_feed_extracts_tag_link_and_date() {
        let body = br#"[
          {
            "tag_name": "v0.2.0-dev.41",
            "html_url": "https://github.com/tttnny/dsh-launcher/releases/tag/v0.2.0-dev.41",
            "published_at": "2026-08-27T03:54:57Z"
          },
          {
            "tag_name": "v0.2.0-dev.40",
            "html_url": "https://github.com/tttnny/dsh-launcher/releases/tag/v0.2.0-dev.40",
            "published_at": null
          }
        ]"#;
        let entries = parse_releases_json(body).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].tag_name, "v0.2.0-dev.41");
        assert_eq!(
            entries[0].html_url,
            "https://github.com/tttnny/dsh-launcher/releases/tag/v0.2.0-dev.41"
        );
        assert_eq!(
            entries[0].published_at.as_deref(),
            Some("2026-08-27T03:54:57Z")
        );
        assert_eq!(entries[1].published_at, None);
    }

    #[test]
    fn json_feed_rejects_malformed_body() {
        assert!(parse_releases_json(b"not json").is_err());
    }

    #[test]
    fn stable_channel_ignores_prereleases() {
        let current = semver::Version::parse("0.1.9").unwrap();
        let releases = vec![release("v0.2.0-dev.7"), release("v0.1.9")];
        assert_eq!(tag_of(pick_latest(&current, false, &releases)), None);
    }

    #[test]
    fn stable_channel_finds_newer_stable() {
        let current = semver::Version::parse("0.1.9").unwrap();
        let releases = vec![release("v0.2.0-dev.7"), release("v0.2.0")];
        assert_eq!(
            tag_of(pick_latest(&current, false, &releases)),
            Some("v0.2.0".to_string())
        );
    }

    #[test]
    fn parse_channel_accepts_dev_and_release() {
        let current = semver::Version::parse("0.2.0").unwrap();
        assert!(parse_channel(Some("dev"), &current).unwrap());
        assert!(parse_channel(Some("DEV"), &current).unwrap());
        assert!(!parse_channel(Some("release"), &current).unwrap());
        assert!(parse_channel(Some("stable"), &current).is_err());
    }

    #[test]
    fn installed_rc_checks_for_the_next_rc_by_default() {
        let current = semver::Version::parse("0.0.1-rc.1").unwrap();
        let releases = parse_releases_json(
            br#"[
          {
            "tag_name": "v0.0.1-rc.2",
            "html_url": "https://github.com/tttnny/dsh-launcher/releases/tag/v0.0.1-rc.2",
            "draft": false,
            "prerelease": false,
            "assets": [{
              "name": "dsh-launcher_0.0.1-rc.2_universal.dmg",
              "size": 10664909,
              "label": "Signed macOS installer",
              "digest": "sha256:56b1efbbab39b8f4a25b69b2d79b1860f089ec49029d8826b5f7afc5e1581683"
            }]
          }
        ]"#,
        )
        .unwrap();
        let channel = parse_channel(None, &current).unwrap();
        let latest = pick_latest(&current, channel, &releases).unwrap();
        assert_eq!(latest.tag_name, "v0.0.1-rc.2");
        for arch in ["aarch64", "x86_64"] {
            assert_eq!(mac_installer(latest, arch).unwrap().0.size, 10664909);
        }
        let stable = semver::Version::parse("0.0.1").unwrap();
        assert!(!parse_channel(None, &stable).unwrap());
        let mut legacy = releases.into_iter().next().unwrap();
        legacy.assets[0].label = None;
        assert!(mac_installer(&legacy, "aarch64").is_none());
    }

    #[test]
    fn dev_channel_finds_newer_dev_build() {
        let current = semver::Version::parse("0.2.0-dev.3").unwrap();
        let releases = vec![release("v0.2.0-dev.2"), release("v0.2.0-dev.5")];
        assert_eq!(
            tag_of(pick_latest(&current, true, &releases)),
            Some("v0.2.0-dev.5".to_string())
        );
    }

    #[test]
    fn dev_channel_prefers_shipped_stable_over_dev_build() {
        let current = semver::Version::parse("0.2.0-dev.9").unwrap();
        let releases = vec![release("v0.2.0")];
        assert_eq!(
            tag_of(pick_latest(&current, true, &releases)),
            Some("v0.2.0".to_string())
        );
    }

    #[test]
    fn same_version_and_unparsable_tags_are_excluded() {
        let current = semver::Version::parse("0.2.0-dev.9").unwrap();
        let releases = vec![release("v0.2.0-dev.9"), release("garbage-tag")];
        assert_eq!(tag_of(pick_latest(&current, true, &releases)), None);
    }
}
