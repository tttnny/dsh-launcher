use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

use crate::AppState;

#[derive(Clone, Debug, Serialize)]
pub struct ToolStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeStatus {
    pub node: ToolStatus,
    pub pnpm: ToolStatus,
}

async fn probe(program: &str) -> ToolStatus {
    // GUI-launched apps inherit a minimal PATH with no shell rc, so a plain
    // Command::new(program) misses fnm/Homebrew/user-installed tools. Walk the
    // PATH entries ourselves, then fall back to the known tool directories
    // (fnm layouts, ~/Library/pnpm, Homebrew, …).
    let mut candidates: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    for dir in tool_fallback_bins() {
        if !candidates.contains(&dir) {
            candidates.push(dir);
        }
    }

    for dir in candidates {
        let exe = if dir.as_os_str().is_empty() {
            PathBuf::from(program)
        } else {
            dir.join(program)
        };
        if !exe.is_file() {
            continue;
        }
        let mut cmd = tokio::process::Command::new(&exe);
        crate::process::hide_console(&mut cmd);
        if let Ok(out) = cmd.arg("--version").output().await {
            if out.status.success() {
                let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return ToolStatus {
                    installed: true,
                    version: Some(version),
                    path: Some(exe.to_string_lossy().into_owned()),
                };
            }
        }
    }
    ToolStatus {
        installed: false,
        version: None,
        path: None,
    }
}

#[tauri::command]
pub async fn get_runtime_status(_state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    let node = probe("node").await;
    let pnpm = probe("pnpm").await;
    Ok(RuntimeStatus { node, pnpm })
}

// ---------------------------------------------------------------------------
// One-click Node.js runtime (issue #23)
// ---------------------------------------------------------------------------

/// The official dist index; npmmirror as the fallback for restricted
/// networks (the configured launcher proxy applies to both).
const NODE_DIST_PRIMARY: &str = "https://nodejs.org/dist";
const NODE_DIST_MIRROR: &str = "https://registry.npmmirror.com/-/binary/node";

/// The managed runtime lives in `<data>/tools/node`.
fn local_node_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tools").join("node")
}

fn local_node_exe(node_dir: &Path) -> PathBuf {
    node_dir.join("bin").join("node")
}

/// Directory containing node/npm/npx shims for PATH purposes.
fn local_node_bin_dir(node_dir: &Path) -> PathBuf {
    node_dir.join("bin")
}

/// fnm-managed Node bin directories, most specific first:
///
/// 1. the `default` alias (`<fnm-root>/aliases/default/bin`)
/// 2. `<fnm-root>/current/bin`
/// 3. every `<fnm-root>/node-versions/<v>/installation/bin`, newest first.
///
/// Both the modern XDG layout (`~/.local/share/fnm`) and the legacy layout
/// (`~/.fnm`) are scanned.
fn fnm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in [
        home.join(".fnm"),
        home.join(".local").join("share").join("fnm"),
    ] {
        let alias = root.join("aliases").join("default").join("bin");
        if alias.is_dir() && !dirs.contains(&alias) {
            dirs.push(alias);
        }
        let current = root.join("current").join("bin");
        if current.is_dir() && !dirs.contains(&current) {
            dirs.push(current);
        }
        let versions = root.join("node-versions");
        if let Ok(rd) = std::fs::read_dir(&versions) {
            let mut found: Vec<(semver::Version, PathBuf)> = rd
                .filter_map(|e| {
                    let e = e.ok()?;
                    let name = e.file_name().to_string_lossy().into_owned();
                    let ver = name
                        .strip_prefix('v')
                        .and_then(|s| semver::Version::parse(s).ok())?;
                    Some((ver, e.path()))
                })
                .collect();
            found.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
            for (_, p) in found {
                let bin = p.join("installation").join("bin");
                if bin.is_dir() && !dirs.contains(&bin) {
                    dirs.push(bin);
                }
            }
        }
    }
    dirs
}

/// Known directories where user-level tools (node/pnpm via fnm, standalone
/// pnpm, bun, Homebrew) may live — used as a probe fallback when PATH alone
/// would miss them.
fn tool_fallback_bins() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut dirs = fnm_bin_dirs(&home);
    for d in [
        home.join("Library").join("pnpm"),
        home.join(".bun").join("bin"),
        home.join(".cargo").join("bin"),
        home.join(".local").join("bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ] {
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// Ensures the standard macOS tool paths — fnm-managed Node, Homebrew, and
/// user bin directories — are on PATH even when launched from Finder without
/// an interactive shell, so node/pnpm/git resolve for spawned children.
pub fn ensure_macos_paths() {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut parts: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();

    let mut extra: Vec<PathBuf> = Vec::new();
    if !home.as_os_str().is_empty() {
        for d in [
            home.join(".local").join("bin"),
            home.join("Library").join("pnpm"),
            home.join(".bun").join("bin"),
            home.join(".cargo").join("bin"),
        ] {
            extra.push(d);
        }
        extra.extend(fnm_bin_dirs(&home));
    }
    for d in [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        extra.push(PathBuf::from(d));
    }

    for p in extra {
        if p.is_dir() && !parts.contains(&p) {
            parts.push(p);
        }
    }
    if let Ok(joined) = std::env::join_paths(parts) {
        std::env::set_var("PATH", joined);
    }
}

/// Appends the managed Node.js bin dir to this process's PATH when a local
/// runtime exists. Appending (not prepending) keeps a system Node preferred;
/// child processes (CLI / npm / pnpm / instances) inherit the value. Called
/// once at startup and after a one-click install.
pub fn ensure_local_node_on_path(data_dir: &Path) {
    let node_dir = local_node_dir(data_dir);
    if !local_node_exe(&node_dir).is_file() {
        return;
    }
    let bin = local_node_bin_dir(&node_dir);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut parts: Vec<PathBuf> = std::env::split_paths(&path).collect();
    if parts.contains(&bin) {
        return;
    }
    parts.push(bin.clone());
    if let Ok(joined) = std::env::join_paths(parts) {
        std::env::set_var("PATH", joined);
        crate::log_info!("已将内置 Node.js 加入 PATH: {}", bin.display());
    }
}

/// dist archive file name for macOS aarch64 (Apple Silicon).
fn node_archive_name(version: &str) -> String {
    format!("node-{version}-darwin-arm64.tar.gz")
}

/// Latest LTS version (e.g. `v22.14.0`) from the dist index, primary source
/// first with the mirror as fallback.
async fn resolve_node_version() -> Result<String, String> {
    for base in [NODE_DIST_PRIMARY, NODE_DIST_MIRROR] {
        match crate::plugins::fetch_json(&format!("{base}/index.json"), 8 * 1024 * 1024).await {
            Ok(doc) => {
                if let Some(arr) = doc.as_array() {
                    for rel in arr {
                        let is_lts = rel
                            .get("lts")
                            .map(|l| l.is_string() || l.as_bool().unwrap_or(false))
                            .unwrap_or(false);
                        if !is_lts {
                            continue;
                        }
                        if let Some(v) = rel.get("version").and_then(|v| v.as_str()) {
                            return Ok(v.to_string());
                        }
                    }
                }
                crate::log_warn!("Node.js 版本列表格式异常（{base}），尝试镜像");
            }
            Err(e) => crate::log_warn!("获取 Node.js 版本列表失败（{base}）: {e}，尝试镜像"),
        }
    }
    Err("获取 Node.js 版本列表失败（官方源与镜像均不可用）".to_string())
}

/// Downloads the dist archive for `version` to `dest`, streaming progress
/// into the task (5% → 80%). Falls back to the mirror on failure.
async fn download_node_archive(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    version: &str,
    dest: &Path,
) -> Result<(), String> {
    let name = node_archive_name(version);
    let client = crate::proxy::apply(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(900))
        .user_agent("dsh-launcher")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let mut last_err = String::new();
    for base in [NODE_DIST_PRIMARY, NODE_DIST_MIRROR] {
        let url = format!("{base}/{version}/{name}");
        crate::tasks::push_task_log(app, state, task_id, &format!("下载 {url}")).await;
        match download_one(&client, &url, dest, app, task_id).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                crate::log_warn!("Node.js 下载失败（{base}）: {e}");
                std::fs::remove_file(dest).ok();
                last_err = e;
            }
        }
    }
    Err(format!("下载 Node.js 失败: {last_err}"))
}

async fn download_one(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    app: &AppHandle,
    task_id: &str,
) -> Result<(), String> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest).map_err(|e| format!("创建下载文件失败: {e}"))?;
    let mut done: u64 = 0;
    let mut last_pct = 5u32;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("下载中断: {e}"))? {
        std::io::Write::write_all(&mut file, &chunk)
            .map_err(|e| format!("写入下载文件失败: {e}"))?;
        done += chunk.len() as u64;
        if let Some(ratio) = done.saturating_mul(75).checked_div(total) {
            let pct = 5 + ratio as u32;
            if pct > last_pct {
                last_pct = pct;
                let shown = done.saturating_mul(100).checked_div(total).unwrap_or(0);
                crate::tasks::emit_progress(
                    app,
                    task_id,
                    crate::tasks::TaskState::Running,
                    pct,
                    Some(format!("正在下载 Node.js（{shown}%）")),
                    None,
                );
            }
        }
    }
    Ok(())
}

/// Extracts the dist archive into `node_dir`, stripping the top-level
/// `node-vX-…/` component. Any previous managed runtime is replaced.
fn extract_node_archive(archive: &Path, node_dir: &Path) -> Result<(), String> {
    if node_dir.exists() {
        std::fs::remove_dir_all(node_dir).map_err(|e| format!("清理旧 Node.js 目录失败: {e}"))?;
    }
    std::fs::create_dir_all(node_dir).map_err(|e| format!("创建 Node.js 目录失败: {e}"))?;

    let file = std::fs::File::open(archive).map_err(|e| format!("打开安装包失败: {e}"))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries().map_err(|e| format!("读取安装包失败: {e}"))? {
        let mut entry = entry.map_err(|e| format!("读取安装包条目失败: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("读取安装包条目名失败: {e}"))?
            .into_owned();
        let clean: PathBuf = path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect();
        let rel: PathBuf = clean.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = node_dir.join(rel);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&target).ok();
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
        }
        if kind.is_symlink() {
            // Node dist ships bin/npm, bin/npx and bin/corepack as symlinks
            // into lib/node_modules; materialize them so PATH shims work.
            let link = entry
                .link_name()
                .map_err(|e| format!("读取链接目标失败: {e}"))?
                .ok_or_else(|| "符号链接缺少目标".to_string())?;
            std::os::unix::fs::symlink(&link, &target)
                .map_err(|e| format!("创建符号链接失败 {target:?}: {e}"))?;
            continue;
        }
        if kind.is_file() {
            // Preserve the archive's permission bits (node binary must stay
            // executable; macOS extraction defaults would strip it to 0644).
            let mode = entry.header().mode().ok().unwrap_or(0o755) & 0o7777;
            use std::os::unix::fs::PermissionsExt;
            let mut file =
                std::fs::File::create(&target).map_err(|e| format!("创建文件失败: {e}"))?;
            std::io::copy(&mut entry, &mut file).map_err(|e| format!("解压条目失败: {e}"))?;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).ok();
        }
    }
    Ok(())
}

/// Starts the one-click Node.js install as a background task (issue #23):
/// downloads the latest LTS from the official dist (mirror fallback),
/// unpacks it into `<data>/tools/node`, puts it on PATH, then bootstraps the
/// pinned pnpm through the bundled npm.
#[tauri::command]
pub async fn start_install_node_task(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    if probe("node").await.installed {
        return Err("系统已安装 Node.js".to_string());
    }
    {
        let tasks = state.tasks.lock().await;
        if tasks.values().any(|t| {
            t.kind == "install-node" && t.state == crate::tasks::TaskState::Running
        }) {
            return Err("Node.js 安装任务已在进行".to_string());
        }
    }

    let task = crate::tasks::TaskInfo {
        id: crate::config::new_id("t"),
        kind: "install-node".to_string(),
        label: "一键安装 Node.js 运行时".to_string(),
        version: String::new(),
        state: crate::tasks::TaskState::Running,
        percent: 0,
        created_at: crate::tasks::now_millis(),
        message: None,
        instance_id: None,
        instance_name: None,
        reserved_home_path: None,
        logs: Vec::new(),
        child: None,
    };
    let task_id = task.id.clone();
    state.tasks.lock().await.insert(task_id.clone(), task);
    crate::tasks::emit_progress(
        &app,
        &task_id,
        crate::tasks::TaskState::Running,
        0,
        None,
        None,
    );

    let worker_app = app.clone();
    let worker_task_id = task_id.clone();
    tauri::async_runtime::spawn(async move {
        let state = worker_app.state::<AppState>();
        // The task runner owns the completion invariants (cancel-stays-
        // cancelled, capped logs, terminal progress); this body only runs
        // the stages.
        let result = do_install_node(&worker_app, &state, &worker_task_id).await;
        let msg_done = result
            .as_ref()
            .ok()
            .map(|version| format!("Node.js {version} 已就绪"));
        crate::tasks::finish_task(
            &worker_app,
            &state,
            &worker_task_id,
            result.map(|_| ()),
            |task, ()| {
                if let Some(msg) = msg_done {
                    task.message = Some(msg);
                }
            },
        )
        .await;
    });

    Ok(task_id)
}

async fn do_install_node(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
) -> Result<String, String> {
    crate::tasks::push_task_log(app, state, task_id, "正在查询 Node.js 最新 LTS 版本…").await;
    let version = resolve_node_version().await?;
    // Cancellation during version resolution (no child to kill) must stop the
    // task here: the extraction below replaces any existing managed runtime.
    if !crate::tasks::task_is_running(state, task_id).await {
        return Err("任务已取消".to_string());
    }
    {
        let mut tasks = state.tasks.lock().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.version = version.clone();
        }
    }
    crate::tasks::push_task_log(app, state, task_id, &format!("目标版本: {version}")).await;

    let tools = state.data_dir.join("tools");
    std::fs::create_dir_all(&tools).map_err(|e| format!("创建工具目录失败: {e}"))?;
    let archive = tools.join(node_archive_name(&version));
    download_node_archive(app, state, task_id, &version, &archive).await?;

    // A cancel during the download killed the HTTP stream but may leave this
    // worker running; check before the destructive extraction step.
    if !crate::tasks::task_is_running(state, task_id).await {
        std::fs::remove_file(&archive).ok();
        return Err("任务已取消".to_string());
    }

    crate::tasks::emit_progress(
        app,
        task_id,
        crate::tasks::TaskState::Running,
        85,
        Some("正在解压 Node.js…".to_string()),
        None,
    );
    let node_dir = local_node_dir(&state.data_dir);
    extract_node_archive(&archive, &node_dir)?;
    std::fs::remove_file(&archive).ok();

    // Put the managed runtime on PATH for everything we spawn from here on.
    ensure_local_node_on_path(&state.data_dir);

    // Verify the fresh runtime runs.
    let exe = local_node_exe(&node_dir);
    let mut verify = tokio::process::Command::new(&exe);
    crate::process::hide_console(&mut verify);
    let out = verify
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("校验 Node.js 失败: {e}"))?;
    if !out.status.success() {
        return Err("Node.js 安装后无法运行".to_string());
    }
    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
    crate::tasks::push_task_log(app, state, task_id, &format!("Node.js {got} 安装完成")).await;

    // npm ships with Node; bootstrap the pinned pnpm so the whole
    // environment goes green in one click.
    if !crate::tasks::task_is_running(state, task_id).await {
        return Err("任务已取消".to_string());
    }
    crate::tasks::emit_progress(
        app,
        task_id,
        crate::tasks::TaskState::Running,
        92,
        Some("正在安装 pnpm…".to_string()),
        None,
    );
    crate::toolchain::ensure_pnpm_for_task(app, state, task_id).await?;
    crate::tasks::push_task_log(app, state, task_id, "pnpm 已就绪").await;
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree(home: &Path) {
        std::fs::create_dir_all(
            home.join(".local/share/fnm/node-versions/v20.11.0/installation/bin"),
        )
        .unwrap();
        std::fs::create_dir_all(
            home.join(".local/share/fnm/node-versions/v24.16.0/installation/bin"),
        )
        .unwrap();
    }

    #[test]
    fn fnm_dirs_prefer_default_alias_then_newest_version() {
        let home = std::env::temp_dir().join(format!("fnm-test-{}", uuid::Uuid::new_v4()));
        make_tree(&home);
        // No alias: newest installed version's bin wins.
        let dirs = fnm_bin_dirs(&home);
        assert!(!dirs.is_empty());
        assert!(dirs[0].ends_with("node-versions/v24.16.0/installation/bin"));
        // With a default alias, it ranks first.
        let alias = home.join(".local/share/fnm/aliases/default/bin");
        std::fs::create_dir_all(&alias).unwrap();
        let dirs = fnm_bin_dirs(&home);
        assert_eq!(dirs[0], alias);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn fnm_dirs_handles_legacy_root() {
        let home = std::env::temp_dir().join(format!("fnm-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".fnm/current/bin")).unwrap();
        let dirs = fnm_bin_dirs(&home);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with(".fnm/current/bin"));
        std::fs::remove_dir_all(&home).ok();
    }
}
