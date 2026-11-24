//! Launcher self-update check against GitHub releases.
//!
//! Uses the GitHub REST API (`/repos/{repo}/releases`) with the anonymous
//! OAuth client id boost (see `plugins::GITHUB_CLIENT_ID`): plain
//! unauthenticated API calls are limited to 60 req/h per IP, and the
//! `releases.atom` feed served from the main github.com domain is not the API
//! at all — under heavy IP load it can be refused at the transport layer
//! (`error sending request for url ...`). Passing the public client id raises
//! the unauth quota to 5000 req/h, exactly as the plugin marketplace does.
//!
//! By default, prerelease builds follow the preview channel and stable
//! builds follow the stable channel.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tauri::{ipc::Channel, AppHandle, Manager};
use tokio::io::AsyncWriteExt;

/// Self-update follows *this* repository's releases only.
const RELEASES_API: &str = "/repos/tttnny/dsh-launcher/releases?per_page=100";
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;
static INSTALLER_DOWNLOAD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Debug, Serialize)]
pub struct LauncherUpdateInfo {
    pub current: String,
    /// "dev" for the preview channel, otherwise "stable".
    pub channel: String,
    pub up_to_date: bool,
    pub latest: Option<String>,
    pub url: Option<String>,
    pub published_at: Option<String>,
    pub has_installer: bool,
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
}

fn mac_installer<'a>(release: &'a ReleaseEntry, arch: &str) -> Option<(&'a ReleaseAsset, &'a str)> {
    let version = parse_tag(&release.tag_name)?;
    ["universal", arch].into_iter().find_map(|arch| {
        let name = format!("dsh-launcher_{version}_{arch}.dmg");
        let asset = release.assets.iter().find(|asset| asset.name == name)?;
        let hash = asset.digest.as_deref()?.strip_prefix("sha256:")?;
        (hash.len() == 64
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && (1..=MAX_INSTALLER_BYTES).contains(&asset.size))
        .then_some((asset, hash))
    })
}

/// Parses a release tag ("v0.2.0" / "v0.2.0-dev.12") as semver.
fn parse_tag(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

fn parse_releases_json(body: &[u8]) -> Result<Vec<ReleaseEntry>, String> {
    serde_json::from_slice(body).map_err(|e| format!("解析 GitHub Releases JSON 失败: {e}"))
}

/// Picks the newest published release strictly newer than `current`.
/// `dev_channel` sees prereleases and stables; the release channel sees only
/// stables (pre-releases are ignored).
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
        channel: if dev_channel { "dev" } else { "stable" }.to_string(),
        up_to_date: latest.is_none(),
        latest: latest.map(|r| r.tag_name.trim_start_matches('v').to_string()),
        url: latest.map(|r| r.html_url.clone()),
        published_at: latest.and_then(|r| r.published_at.clone()),
        has_installer: cfg!(target_os = "macos")
            && latest
                .and_then(|r| mac_installer(r, std::env::consts::ARCH))
                .is_some(),
    })
}

#[tauri::command]
pub async fn download_launcher_update(
    app: AppHandle,
    version: String,
    on_progress: Channel<u8>,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("当前平台没有可用的启动器安装包".into());
    }
    let _download = INSTALLER_DOWNLOAD
        .try_lock()
        .map_err(|_| "启动器安装包正在下载".to_string())?;
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
    // The tag and filename were matched against SemVer and our bundle naming
    // scheme. The webview never supplies a URL, checksum, or local path.
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
    open::that(&path).map_err(|e| format!("打开安装包失败: {e}"))?;
    Ok(())
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
        // Only prereleases are newer: a stable build stays put.
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
        let releases = vec![
            release("v0.2.0-dev.9"), // same as current
            release("garbage-tag"),  // unparsable
        ];
        assert_eq!(tag_of(pick_latest(&current, true, &releases)), None);
    }
}
