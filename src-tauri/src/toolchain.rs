//! Toolchain module — the single owner of "which pnpm, which store, which
//! registry, which network flags".
//!
//! Every pnpm/npm invocation the launcher drives (version installs, source
//! builds, plugin management, version listings) derives its toolchain policy
//! from here, so a policy change (a new pnpm major, different timeouts, a
//! mirror variable) is made once and applies everywhere. Two entry flavors
//! resolve the pinned pnpm: background tasks get task-log notes and a percent
//! nudge ([`ensure_pnpm_for_task`]); interactive paths just app-log
//! ([`ensure_pnpm`]). They share the probe and bootstrap decisions.

use crate::AppState;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

/// DSH profiles are initialized by pnpm 11, and `dsh plugin` shells out to
/// whatever pnpm is on PATH. A different pnpm major produces trees the CLI
/// does not expect and fails in ways that look unrelated, so the launcher
/// pins the major it drives every install with.
pub(crate) const REQUIRED_PNPM_MAJOR: u32 = 11;

/// Parses the major version out of `pnpm --version` output ("11.17.0\n").
pub(crate) fn pnpm_major(version_output: &str) -> Option<u32> {
    version_output
        .trim()
        .split('.')
        .next()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Optional registry mirror (e.g. npmmirror) via `DSH_NPM_REGISTRY`, trimmed.
/// Shared by every npm/pnpm invocation so listings and installs never
/// disagree about which registry answered.
pub(crate) fn registry_mirror() -> Option<String> {
    std::env::var("DSH_NPM_REGISTRY")
        .ok()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
}

/// The launcher's shared pnpm content store under the data dir, reused by
/// version installs, source builds, and plugin management. Profiles linked
/// against any other store fail with ERR_PNPM_UNEXPECTED_STORE.
pub(crate) fn store_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(".pnpm-store")
}

/// Network robustness flags for pnpm *download* subcommands (`add` /
/// `install`): the defaults (60s timeout, 2 retries) are too tight for large
/// native binaries (e.g. sharp), which abort on flaky connections. `pnpm
/// remove` rejects these flags outright, so download-capable commands append
/// them themselves.
pub(crate) fn pnpm_fetch_flags() -> [&'static str; 8] {
    [
        "--fetch-timeout",
        "300000", // 5 min per request
        "--fetch-retries",
        "5",
        "--fetch-retry-maxtimeout",
        "120000",
        "--network-concurrency",
        "4",
    ]
}

/// Path of the pinned pnpm executable inside the data dir's tools dir.
fn local_pnpm_path(tools_dir: &Path) -> PathBuf {
    tools_dir.join("pnpm")
}

/// Runs `<prog> --version`; `Some((raw_output, major))` when the probe
/// succeeded (the major is `None` when the output is not a version).
async fn probe_pnpm_version(prog: &Path) -> Option<(String, Option<u32>)> {
    let mut cmd = tokio::process::Command::new(prog);
    crate::process::hide_console(&mut cmd);
    let out = cmd.arg("--version").output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).to_string();
    let major = pnpm_major(&raw);
    Some((raw, major))
}

/// The shared probe: system pnpm on the required major, else the pinned pnpm
/// already bootstrapped into the data dir. `None` means bootstrap is needed.
/// `note` receives the human-readable reason when a system pnpm exists but is
/// skipped (wrong major).
async fn find_pnpm(
    data_dir: &Path,
    note: &mut (dyn FnMut(String) + Send),
) -> Option<PathBuf> {
    let system = PathBuf::from(crate::process::pnpm());
    if let Some((raw, major)) = probe_pnpm_version(&system).await {
        if major == Some(REQUIRED_PNPM_MAJOR) {
            return Some(system);
        }
        let shown = raw.trim().to_string();
        note(format!(
            "系统 pnpm {shown} 与所需的 pnpm {REQUIRED_PNPM_MAJOR} 不符，使用启动器内置 pnpm"
        ));
    }

    let tools_dir = data_dir.join("tools");
    let local = local_pnpm_path(&tools_dir);
    if local.exists() {
        if let Some((_, major)) = probe_pnpm_version(&local).await {
            if major == Some(REQUIRED_PNPM_MAJOR) {
                return Some(local);
            }
        }
    }
    None
}

/// Bootstraps the pinned pnpm major into `<data>/tools` via npm. The install
/// honors the launcher proxy like every other network step. Callers wrap
/// this with their own flavor of user-facing notes.
async fn bootstrap_pnpm(data_dir: &Path) -> Result<PathBuf, String> {
    let tools_dir = data_dir.join("tools");
    std::fs::create_dir_all(&tools_dir).map_err(|e| format!("创建工具目录失败: {e}"))?;
    let spec = format!("pnpm@{REQUIRED_PNPM_MAJOR}");

    let mut child_cmd = tokio::process::Command::new(crate::process::npm());
    crate::process::hide_console(&mut child_cmd);
    crate::proxy::apply_to_command(&mut child_cmd);
    child_cmd
        .args(["install", "--global", "--prefix"])
        .arg(&tools_dir)
        .arg(&spec)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = child_cmd
        .spawn()
        .map_err(|e| format!("pnpm 安装启动失败: {e}"))?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("pnpm 安装等待失败: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let last = err.lines().last().unwrap_or(&err).to_string();
        return Err(format!("pnpm 安装失败: {last}"));
    }

    let local = local_pnpm_path(&tools_dir);
    if !local.exists() {
        return Err(format!(
            "pnpm 安装完成但未找到可执行文件: {}",
            local.display()
        ));
    }
    Ok(local)
}

/// Resolves a pnpm executable on the required major for interactive paths
/// (plugin management): notes go to the app log.
pub(crate) async fn ensure_pnpm(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    let mut notes = Vec::new();
    if let Some(p) = find_pnpm(&state.data_dir, &mut |s| notes.push(s)).await {
        for note in notes {
            crate::log_info!("{note}");
        }
        return Ok(p);
    }
    crate::log_info!("正在安装 DSH profile 所需的 pnpm@{REQUIRED_PNPM_MAJOR}…");
    bootstrap_pnpm(&state.data_dir).await
}

/// Resolves a pnpm executable on the required major for a background task:
/// mismatch/bootstrap notes also land in the task log, and bootstrap bumps
/// the task percent so the UI shows the extra phase.
pub(crate) async fn ensure_pnpm_for_task(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
) -> Result<PathBuf, String> {
    let mut notes = Vec::new();
    if let Some(p) = find_pnpm(&state.data_dir, &mut |s| notes.push(s)).await {
        for note in &notes {
            crate::log_warn!("{note}");
            crate::tasks::push_task_log(app, state, task_id, note).await;
        }
        return Ok(p);
    }

    let spec = format!("pnpm@{REQUIRED_PNPM_MAJOR}");
    let msg = format!("正在安装 DSH profile 所需的 {spec}…");
    {
        let mut tasks = state.tasks.lock().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.percent = 5;
            crate::tasks::push_log_locked(task, &msg);
        }
    }
    crate::tasks::emit_progress(
        app,
        task_id,
        crate::tasks::TaskState::Running,
        5,
        None,
        None,
    );
    crate::log_info!("引导安装 {spec} 到 {}", state.data_dir.join("tools").display());
    bootstrap_pnpm(&state.data_dir).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnpm_major_parses_version_output() {
        assert_eq!(pnpm_major("11.17.0\n"), Some(11));
        assert_eq!(pnpm_major("  10.4.1  "), Some(10));
        assert_eq!(pnpm_major("12.0.0-beta.1"), Some(12));
        assert_eq!(pnpm_major(""), None);
        assert_eq!(pnpm_major("not-a-version"), None);
    }

    #[test]
    fn required_pnpm_major_is_the_profile_toolchain() {
        // DSH profiles are initialized by pnpm 11; changing this constant
        // means the launcher drives installs with a different major.
        assert_eq!(REQUIRED_PNPM_MAJOR, 11);
    }

    #[test]
    fn fetch_flags_pair_up() {
        // All flags are `--name value` pairs; even count means no dangling flag.
        assert!(pnpm_fetch_flags().len() % 2 == 0);
    }
}
