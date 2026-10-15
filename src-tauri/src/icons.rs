//! Instance icons (issue #8): an instance may carry a custom icon — an
//! http(s) URL or a local image copied into `<home>/icons/<id>.png`. Local
//! images are center-cropped to a 1:1 square and re-encoded as PNG. The
//! launcher icon stays the default and is never copied or exported.

use std::path::{Path, PathBuf};

use crate::config::Host;
use crate::AppState;
use tauri::State;

const ICON_MAX_BYTES: usize = 16 * 1024 * 1024;
const ICON_SIZE: u32 = 256;

/// Where a local icon lives inside the instance's HOME.
pub(crate) fn local_icon_path(home: &Path, instance_id: &str) -> PathBuf {
    home.join("icons").join(format!("{instance_id}.png"))
}

/// Crops the image to a centered 1:1 square and encodes it as PNG.
pub(crate) fn crop_square_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("解析图像失败: {e}"))?;
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let cropped = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let resized = if side > ICON_SIZE {
        cropped.resize_exact(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3)
    } else {
        cropped
    };
    let mut out = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("编码 PNG 失败: {e}"))?;
    Ok(out.into_inner())
}

/// Downloads an icon URL with a size cap.
async fn fetch_icon(url: &str) -> Result<Vec<u8>, String> {
    let client = crate::proxy::apply(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("dsh-launcher")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载图标失败 {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载图标失败 {url}: HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取图标失败: {e}"))?;
    if bytes.len() > ICON_MAX_BYTES {
        return Err("图标文件过大（超过 16 MiB）".to_string());
    }
    Ok(bytes.to_vec())
}

/// Validates that bytes decode as an image (used to sanity-check remote URLs
/// that stay remote).
fn ensure_decodable(bytes: &[u8]) -> Result<(), String> {
    image::load_from_memory(bytes)
        .map(|_| ())
        .map_err(|e| format!("不是有效的图像文件: {e}"))
}

/// Which machine an instance's icon lives on, and the HOME directory it lives
/// inside: a path on 本机, or a path on that server.
fn instance_scope(state: &AppState, instance_id: &str) -> Result<(Host, PathBuf), String> {
    let cfg = state.config.lock().unwrap();
    let inst = cfg
        .instances
        .iter()
        .find(|i| i.id == instance_id)
        .ok_or_else(|| "实例不存在".to_string())?;
    let home = cfg
        .homes
        .iter()
        .find(|h| h.id == inst.home_id)
        .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
    let host = crate::config::resolve_host(&cfg, Some(&home.host_id))?.clone();
    Ok((host, home.path.clone()))
}

/// How one PNG reaches the page.
fn png_data_url(bytes: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    )
}

/// The icons this session has already read off another machine, with the
/// instance's own `icon` field as the key into them.
///
/// The avatar calls [`read_instance_icon`] once per render and a board has a
/// dozen rows: without this, every row would be an ssh round trip, every time
/// the list is redrawn. The entry is dropped the moment the icon is set or
/// cleared for that instance — the two ways its bytes can change — so a hit is
/// the same answer as the machine would give, never a stale one. Runtime only:
/// what this session has seen, not a record.
static REMOTE_ICON_CACHE: std::sync::Mutex<Vec<CachedIcon>> = std::sync::Mutex::new(Vec::new());

struct CachedIcon {
    instance_id: String,
    /// The `icon` field this was read under, so an entry cannot answer for a
    /// different icon.
    icon: String,
    data: Option<String>,
}

fn cached_icon(instance_id: &str, icon: &str) -> Option<Option<String>> {
    REMOTE_ICON_CACHE
        .lock()
        .unwrap()
        .iter()
        .find(|entry| entry.instance_id == instance_id && entry.icon == icon)
        .map(|entry| entry.data.clone())
}

fn remember_icon(instance_id: &str, icon: &str, data: Option<String>) {
    let mut cache = REMOTE_ICON_CACHE.lock().unwrap();
    cache.retain(|entry| entry.instance_id != instance_id);
    cache.push(CachedIcon {
        instance_id: instance_id.to_string(),
        icon: icon.to_string(),
        data,
    });
}

fn forget_icon(instance_id: &str) {
    REMOTE_ICON_CACHE
        .lock()
        .unwrap()
        .retain(|entry| entry.instance_id != instance_id);
}

/// Sets an instance icon from a local image file or an http(s) URL. Local
/// files are cropped to a square PNG inside the HOME — on this Mac, or, for an
/// instance on another machine, **there**: the image is picked and cropped
/// locally and only the finished PNG travels (user story 35). URLs are stored
/// as-is after a decode sanity check.
#[tauri::command]
pub async fn set_instance_icon(
    state: State<'_, AppState>,
    instance_id: String,
    source: String,
) -> Result<(), String> {
    set_instance_icon_for(&state, &instance_id, &source).await
}

pub(crate) async fn set_instance_icon_for(
    state: &AppState,
    instance_id: &str,
    source: &str,
) -> Result<(), String> {
    let source = source.trim().to_string();
    if source.is_empty() {
        return Err("图标来源不能为空".to_string());
    }
    let (host, home) = instance_scope(state, instance_id)?;

    let icon = if source.starts_with("https://") || source.starts_with("http://") {
        ensure_decodable(&fetch_icon(&source).await?)?;
        source
    } else {
        let src = PathBuf::from(&source);
        let bytes =
            std::fs::read(&src).map_err(|e| format!("读取图标文件失败 {}: {e}", src.display()))?;
        if bytes.len() > ICON_MAX_BYTES {
            return Err("图标文件过大（超过 16 MiB）".to_string());
        }
        let png = crop_square_png(&bytes)?;
        let dest = local_icon_path(&home, instance_id);
        if host.builtin {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建图标目录失败: {e}"))?;
            }
            std::fs::write(&dest, png).map_err(|e| format!("写入图标失败: {e}"))?;
        } else {
            // The same file, at the same place inside that instance's HOME —
            // only the filesystem on the other end of it is different.
            let remote =
                crate::ssh::RemoteHost::of(&host, &state.data_dir).map_err(|e| e.message)?;
            state
                .host_transport
                .write_file(&remote, &dest.to_string_lossy(), png)
                .await
                .map_err(|e| format!("把图标上传到「{}」失败：{e}", host.display_name))?;
        }
        "local".to_string()
    };

    {
        let mut cfg = state.config.lock().unwrap();
        let inst = cfg
            .instances
            .iter_mut()
            .find(|i| i.id == instance_id)
            .ok_or_else(|| "实例不存在".to_string())?;
        inst.icon = Some(icon);
        crate::commands::save_state(state, &cfg)?;
    }
    forget_icon(instance_id);
    Ok(())
}

/// Restores the default launcher icon and removes any local icon file — from
/// this Mac, or from the machine the instance runs on.
#[tauri::command]
pub async fn clear_instance_icon(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<(), String> {
    clear_instance_icon_for(&state, &instance_id).await
}

pub(crate) async fn clear_instance_icon_for(
    state: &AppState,
    instance_id: &str,
) -> Result<(), String> {
    let (host, home) = instance_scope(state, instance_id)?;
    let was_local = {
        let mut cfg = state.config.lock().unwrap();
        let inst = cfg
            .instances
            .iter_mut()
            .find(|i| i.id == instance_id)
            .ok_or_else(|| "实例不存在".to_string())?;
        let was_local = inst.icon.as_deref() == Some("local");
        inst.icon = None;
        crate::commands::save_state(state, &cfg)?;
        was_local
    };
    forget_icon(instance_id);
    if was_local {
        let dest = local_icon_path(&home, instance_id);
        if host.builtin {
            let _ = std::fs::remove_file(&dest);
        } else if let Ok(remote) = crate::ssh::RemoteHost::of(&host, &state.data_dir) {
            // Best effort, exactly as on this Mac: the record is what the page
            // reads, and a machine that is off must not block the clear.
            let _ = crate::remote::remove_dir(
                &*state.host_transport,
                &remote,
                &dest.to_string_lossy(),
            )
            .await;
        }
    }
    Ok(())
}

/// Resolves an instance icon for display: remote URLs pass through, local
/// files become a `data:` URL, and `None` means the launcher default.
///
/// For an instance on another machine the file is read over the transport and
/// kept in [`REMOTE_ICON_CACHE`], so a redrawn board costs no further calls.
#[tauri::command]
pub async fn read_instance_icon(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<Option<String>, String> {
    read_instance_icon_for(&state, &instance_id).await
}

pub(crate) async fn read_instance_icon_for(
    state: &AppState,
    instance_id: &str,
) -> Result<Option<String>, String> {
    let (host, home) = instance_scope(state, instance_id)?;
    let icon = {
        let cfg = state.config.lock().unwrap();
        cfg.instances
            .iter()
            .find(|i| i.id == instance_id)
            .and_then(|i| i.icon.clone())
    };
    match icon.as_deref() {
        None => Ok(None),
        Some("local") => {
            let dest = local_icon_path(&home, instance_id);
            if host.builtin {
                let bytes = std::fs::read(&dest).map_err(|e| format!("读取图标失败: {e}"))?;
                return Ok(Some(png_data_url(&bytes)));
            }
            if let Some(cached) = cached_icon(instance_id, "local") {
                return Ok(cached);
            }
            let remote =
                crate::ssh::RemoteHost::of(&host, &state.data_dir).map_err(|e| e.message)?;
            let bytes = state
                .host_transport
                .read_file(&remote, &dest.to_string_lossy())
                .await
                .map_err(|e| format!("读取「{}」上的图标失败：{e}", host.display_name))?;
            let data = Some(png_data_url(&bytes));
            remember_icon(instance_id, "local", data.clone());
            Ok(data)
        }
        Some(url) => Ok(Some(url.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4x2 red rectangle PNG, generated once for the crop test.
    fn rect_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(4, 2, image::Rgb([255, 0, 0]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn crop_square_centers_wide_images() {
        let png = crop_square_png(&rect_png()).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), img.height());
        assert_eq!(img.width(), 2);
    }

    // -----------------------------------------------------------------------
    // Icons on another machine
    // -----------------------------------------------------------------------

    struct ScratchDir(std::path::PathBuf);
    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("dsh-{tag}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            ScratchDir(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn remote_host() -> Host {
        Host {
            id: "host-lab".into(),
            display_name: "服务器".into(),
            ssh_target: "zjq_aliyun".into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: Some(std::path::PathBuf::from(
                "/home/demo/.local/share/dsh-launcher",
            )),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    /// One instance, in a HOME on `host`, with the transport a test chose.
    fn state_with(
        data: &ScratchDir,
        transport: std::sync::Arc<dyn crate::ssh::HostTransport>,
        host: Host,
        home_path: std::path::PathBuf,
        instance_id: &str,
    ) -> AppState {
        AppState {
            config_path: data.path().join("config.json"),
            data_dir: data.path().to_path_buf(),
            config: std::sync::Mutex::new(crate::config::Config {
                hosts: vec![Host::local(), host.clone()],
                homes: vec![crate::config::DshHome {
                    id: "h1".into(),
                    name: "默认".into(),
                    path: home_path,
                    host_id: host.id.clone(),
                }],
                instances: vec![crate::config::DshInstance {
                    id: instance_id.into(),
                    name: "实例".into(),
                    version_id: "v1".into(),
                    home_id: "h1".into(),
                    host_id: host.id.clone(),
                    env_overrides: Default::default(),
                    default_profile: None,
                    last_profile: None,
                    icon: None,
                    port: None,
                }],
                ..Default::default()
            }),
            running: tokio::sync::Mutex::new(Default::default()),
            tasks: tokio::sync::Mutex::new(Default::default()),
            host_transport: transport,
            host_statuses: std::sync::Mutex::new(Default::default()),
            remote_scan: std::sync::Mutex::new(Default::default()),
            forwards: Default::default(),
        }
    }

    /// The image is picked and cropped on this Mac and only the finished PNG
    /// travels: it lands at the same place inside the instance's HOME — on the
    /// machine that owns the HOME (user story 35).
    #[test]
    fn a_remote_instances_icon_is_uploaded_to_that_machine() {
        use crate::remote::testing::{FakeAnswer, FakeCall, FakeTransport};

        let data = ScratchDir::new("icon-remote-upload");
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = state_with(
            &data,
            fake.clone(),
            remote_host(),
            std::path::PathBuf::from("/home/demo/.dsh"),
            "i-icon-upload",
        );
        let src = data.path().join("picked.png");
        std::fs::write(&src, rect_png()).unwrap();

        block_on(set_instance_icon_for(
            &state,
            "i-icon-upload",
            &src.to_string_lossy(),
        ))
        .unwrap();

        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        match &calls[0] {
            FakeCall::WriteFile(path, bytes) => {
                assert_eq!(path, "/home/demo/.dsh/icons/i-icon-upload.png");
                let written = image::load_from_memory(bytes).unwrap();
                assert_eq!(written.width(), written.height(), "上传的是裁成方形的 PNG");
            }
            other => panic!("图标要走写文件那次调用：{other:?}"),
        }
        assert_eq!(
            state.config.lock().unwrap().instances[0].icon.as_deref(),
            Some("local")
        );
    }

    /// A row reads its icon on every render, and a board has a dozen rows: the
    /// bytes are kept, so the machine is asked once rather than once per
    /// redraw — and dropped the moment the icon changes, so a hit is never a
    /// stale answer.
    #[test]
    fn a_remote_icons_bytes_are_read_once_and_then_cached() {
        use crate::remote::testing::{FakeAnswer, FakeCall, FakeTransport};

        let data = ScratchDir::new("icon-remote-cache");
        let png = crop_square_png(&rect_png()).unwrap();
        let fake = FakeTransport::new()
            .answering(FakeAnswer::Done)
            .answering(FakeAnswer::Bytes(png.clone()))
            .answering(FakeAnswer::Done)
            .answering(FakeAnswer::Bytes(png));
        let state = state_with(
            &data,
            fake.clone(),
            remote_host(),
            std::path::PathBuf::from("/home/demo/.dsh"),
            "i-icon-cache",
        );
        let src = data.path().join("picked.png");
        std::fs::write(&src, rect_png()).unwrap();
        block_on(set_instance_icon_for(
            &state,
            "i-icon-cache",
            &src.to_string_lossy(),
        ))
        .unwrap();

        let first = block_on(read_instance_icon_for(&state, "i-icon-cache")).unwrap();
        let second = block_on(read_instance_icon_for(&state, "i-icon-cache")).unwrap();

        assert_eq!(first, second, "同一个图标两次渲染是同一个答案");
        assert!(
            first.as_deref().unwrap_or("").starts_with("data:image/png;base64,"),
            "页面拿到的是图本身：{first:?}"
        );
        let reads = |fake: &FakeTransport| {
            fake.calls()
                .iter()
                .filter(|call| matches!(call, FakeCall::ReadFile(_)))
                .count()
        };
        assert_eq!(reads(&fake), 1, "两次渲染只问那台机器一次");

        // Choosing a new image drops the cached copy: the next render asks for
        // the new file instead of showing the old one.
        block_on(set_instance_icon_for(
            &state,
            "i-icon-cache",
            &src.to_string_lossy(),
        ))
        .unwrap();
        block_on(read_instance_icon_for(&state, "i-icon-cache")).unwrap();
        assert_eq!(reads(&fake), 2, "换了图标就要重新读");
    }

    /// 本机's icon stays exactly what it was: the file inside its own HOME,
    /// read directly, with no ssh in a row that lives on this Mac.
    #[test]
    fn a_local_instances_icon_is_still_the_file_in_its_own_home() {
        use crate::remote::testing::FakeTransport;

        let data = ScratchDir::new("icon-local");
        let fake = FakeTransport::new();
        let home = data.path().join("home");
        let state = state_with(&data, fake.clone(), Host::local(), home.clone(), "i-icon-local");
        let src = data.path().join("picked.png");
        std::fs::write(&src, rect_png()).unwrap();

        block_on(set_instance_icon_for(
            &state,
            "i-icon-local",
            &src.to_string_lossy(),
        ))
        .unwrap();

        assert!(local_icon_path(&home, "i-icon-local").is_file());
        let read = block_on(read_instance_icon_for(&state, "i-icon-local"))
            .unwrap()
            .unwrap();
        assert!(read.starts_with("data:image/png;base64,"));
        assert!(fake.calls().is_empty(), "本机的图标不过 ssh：{:?}", fake.calls());
    }
}
