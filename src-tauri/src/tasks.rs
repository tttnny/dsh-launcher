use crate::config::{new_id, DshInstance, DshVersion};
use crate::AppState;
use serde::Serialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

pub const TASK_PROGRESS_EVENT: &str = "task://progress";
pub const TASK_LOG_EVENT: &str = "task://log";

const MAX_LOG_LINES: usize = 1000;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Running,
    Done,
    Error,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskInfo {
    pub id: String,
    pub kind: String, // "create-instance"
    pub label: String,
    pub version: String,
    pub state: TaskState,
    pub percent: u32,
    pub created_at: i64,
    pub message: Option<String>,
    pub instance_id: Option<String>,
    pub instance_name: Option<String>,
    /// Reserved dedicated HOME path while the task is running; the actual
    /// HOME record is only created when the instance is created, so a
    /// cancelled/failed task never leaves an orphan HOME. Not serialized.
    #[serde(skip)]
    pub reserved_home_path: Option<std::path::PathBuf>,
    pub logs: Vec<String>,
    #[serde(skip)]
    pub child: Option<Arc<Mutex<Option<tokio::process::Child>>>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskProgress {
    pub id: String,
    pub state: TaskState,
    pub percent: u32,
    pub message: Option<String>,
    pub instance_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskLog {
    pub id: String,
    pub line: String,
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whether a version string is safe to use as a path segment under
/// `versions/`: npm semver plus the alpha tags upstream publishes. Rejects
/// path separators, `..`, and anything exotic before the string is ever
/// joined into a filesystem path (the Tauri command boundary and GitHub tag
/// names are both untrusted input — a Git tag may legally contain `/`).
pub(crate) fn valid_version_string(v: &str) -> bool {
    // `.`/`..` are charset-legal but are exactly the path-traversal segments.
    !v.is_empty()
        && v != "."
        && v != ".."
        && v.len() <= 64
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
}

/// Whether the task is still allowed to do work. Checked
/// at stage boundaries so a task cancelled during a phase without a
/// registered child (npm probing, pnpm bootstrap, profile boot) stops instead
/// of silently creating the HOME/instance records afterwards.
pub(crate) async fn task_is_running(
    state: &State<'_, AppState>,
    task_id: &str,
) -> bool {
    let tasks = state.tasks.lock().await;
    tasks
        .get(task_id)
        .map(|t| t.state == TaskState::Running)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Enqueues a background task that installs the given DSH version (if not
/// installed yet), auto-creates its dedicated HOME, and registers the 1:1 instance.
#[tauri::command(rename_all = "snake_case")]
pub async fn start_install_version_task(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
) -> Result<String, String> {
    let v = version.trim().to_string();
    start_create_instance_task(app, state, v.clone(), v, None, true).await
}

/// Enqueues a background task that installs the given DSH version (if not
/// installed yet) and then creates the instance. Returns the task id.
/// Internal: only reachable through `start_install_version_task` — installing
/// a version IS creating its 1:1 instance (see ADR-0001).
async fn start_create_instance_task(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    version: String,
    home_id: Option<String>,
    dedicated: bool,
) -> Result<String, String> {
    let name = name.trim().to_string();
    let version = version.trim().to_string();
    if name.is_empty() {
        return Err("实例名称不能为空".to_string());
    }
    if version.is_empty() {
        return Err("版本号不能为空".to_string());
    }
    if !valid_version_string(&version) {
        return Err(format!("版本号格式不合法: {version}"));
    }

    // Dedicated HOME: reserve the path now (placeholder) but do NOT create the
    // HOME record yet — it is created only once the instance is actually made,
    // so a failed/cancelled task leaves no orphan HOME behind.
    let reserved_home_path: Option<std::path::PathBuf> = if dedicated {
        let path = state
            .data_dir
            .join("homes")
            .join(crate::config::sanitize_name(&name));
        Some(path)
    } else {
        None
    };

    // Validate early so a doomed task is never enqueued.
    {
        let cfg = state.config.lock().unwrap();
        if cfg.instances.iter().any(|i| i.name == name) {
            return Err("同名实例已存在".to_string());
        }
        // For a non-dedicated task the chosen HOME must exist already.
        if !dedicated {
            if let Some(hid) = &home_id {
                if !cfg.homes.iter().any(|h| h.id == *hid) {
                    return Err("DSH_HOME 不存在".to_string());
                }
            }
        }
    }
    let task_label = if name == version {
        format!("下载 DSH {version} 并创建实例")
    } else {
        format!("下载 DSH {version} 并创建实例「{name}」")
    };

    // Deduplicate and insert under one lock scope so two concurrent submissions
    // can never both pass the checks (TOCTOU). A running create-instance task
    // for the same version must also be rejected: the version record is only
    // registered when its install finishes, so two installs of the same
    // version would otherwise run `pnpm install --prefix` into the same
    // directory concurrently and corrupt the tree.
    let task_id = {
        let mut tasks = state.tasks.lock().await;
        for task in tasks.values() {
            if task.state == TaskState::Running {
                if task.instance_name.as_deref() == Some(name.as_str()) {
                    return Err("同名实例的下载任务已在进行中".to_string());
                }
                if task.kind == "create-instance" && task.version == version {
                    return Err(format!("版本 {version} 的下载任务已在进行中"));
                }
                if let (Some(a), Some(b)) = (&task.reserved_home_path, &reserved_home_path) {
                    if crate::config::paths_equal(a, b) {
                        return Err("该专属 DSH_HOME 已被其他下载任务占用".to_string());
                    }
                }
            }
        }
        let task = TaskInfo {
            id: new_id("t"),
            kind: "create-instance".to_string(),
            label: task_label,
            version: version.clone(),
            state: TaskState::Running,
            percent: 0,
            created_at: now_millis(),
            message: None,
            instance_id: None,
            instance_name: Some(name.clone()),
            reserved_home_path,
            logs: Vec::new(),
            child: None,
        };
        let id = task.id.clone();
        tasks.insert(id.clone(), task);
        id
    };
    emit_progress(&app, &task_id, TaskState::Running, 0, None, None);

    let worker_app = app.clone();
    let worker_task_id = task_id.clone();
    tauri::async_runtime::spawn(async move {
        let state = worker_app.state::<AppState>();
        run_create_instance_task(
            &worker_app,
            &state,
            &worker_task_id,
            &name,
            &version,
            &home_id,
        )
        .await;
    });

    Ok(task_id)
}

// ---------------------------------------------------------------------------
// Task runner — completion bookkeeping shared by every background task kind
// (create-instance, install-node). A task body runs to `Ok(payload)` /
// `Err(message)`; [`finish_task`] encodes the completion invariants once:
// a cancelled task stays cancelled (the worker finishing anyway must not
// flip it back), logs cap at MAX_LOG_LINES, and terminal progress always
// reaches the frontend.
// ---------------------------------------------------------------------------

/// Writes a task body's result into the task record and emits the terminal
/// progress. `on_done` maps the success payload to extra bookkeeping (e.g.
/// the created instance id, HOME reservation release).
pub(crate) async fn finish_task<T>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    result: Result<T, String>,
    on_done: impl FnOnce(&mut TaskInfo, T),
) {
    let mut tasks = state.tasks.lock().await;
    let Some(task) = tasks.get_mut(task_id) else {
        return;
    };
    if task.state == TaskState::Cancelled {
        return;
    }
    match result {
        Ok(payload) => {
            task.state = TaskState::Done;
            task.percent = 100;
            on_done(task, payload);
            emit_progress(app, task_id, TaskState::Done, 100, None, task.instance_id.clone());
        }
        Err(msg) => {
            task.state = TaskState::Error;
            task.message = Some(msg.clone());
            push_log_locked(task, &format!("error: {msg}"));
            let pct = task.percent;
            emit_progress(app, task_id, TaskState::Error, pct, Some(msg), None);
        }
    }
}

#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Vec<TaskInfo>, String> {
    let tasks = state.tasks.lock().await;
    let mut out: Vec<TaskInfo> = tasks.values().cloned().collect();
    out.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    Ok(out)
}

#[tauri::command]
pub async fn remove_task(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut tasks = state.tasks.lock().await;
    let Some(task) = tasks.get(&id) else {
        return Err("任务不存在".to_string());
    };
    if task.state == TaskState::Running {
        return Err("任务仍在运行中，请先取消".to_string());
    }
    tasks.remove(&id);
    Ok(())
}

#[tauri::command]
pub async fn cancel_task(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let child = {
        let tasks = state.tasks.lock().await;
        tasks.get(&id).and_then(|t| t.child.clone())
    };
    {
        let mut tasks = state.tasks.lock().await;
        if let Some(task) = tasks.get_mut(&id) {
            task.state = TaskState::Cancelled;
            task.message = Some("已取消".to_string());
        }
    }
    if let Some(child) = child {
        let taken = child.lock().await.take();
        if let Some(mut c) = taken {
            let _ = c.kill().await;
        }
    }
    emit_progress(
        &app,
        &id,
        TaskState::Cancelled,
        0,
        Some("已取消".to_string()),
        None,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

async fn run_create_instance_task(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    name: &str,
    version: &str,
    home_id: &Option<String>,
) {
    // The dedicated HOME path is read from the task's reservation; only then
    // is the actual HOME record created (inside do_create_instance).
    let reserved = {
        let tasks = state.tasks.lock().await;
        tasks
            .get(task_id)
            .and_then(|t| t.reserved_home_path.clone())
    };
    let result = do_create_instance(
        app,
        state,
        task_id,
        name,
        version,
        home_id,
        reserved.as_deref(),
    )
    .await;

    if let Ok(instance_id) = &result {
        crate::log_info!("任务 {task_id} 完成，实例 {instance_id} 已创建");
    }
    if let Err(msg) = &result {
        crate::log_error!("任务 {task_id} 失败：{msg}");
    }
    finish_task(app, state, task_id, result, |task, instance_id| {
        task.instance_id = Some(instance_id);
        // The dedicated HOME now exists for real; release the placeholder.
        task.reserved_home_path = None;
    })
    .await;
}

async fn do_create_instance(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    name: &str,
    version: &str,
    home_id: &Option<String>,
    reserved_home_path: Option<&std::path::Path>,
) -> Result<String, String> {
    // 1. Install the version if missing.
    let version_record = {
        let cfg = state.config.lock().unwrap();
        cfg.versions.iter().find(|v| v.version == version).cloned()
    };
    let version_record = match version_record {
        Some(v) => v,
        None => {
            if !task_is_running(state, task_id).await {
                return Err("任务已取消".to_string());
            }
            install_version_streamed(app, state, task_id, version).await?
        }
    };

    // Cancellation during a stage without a registered child (npm probing,
    // pnpm bootstrap) must not fall through into record creation: the task
    // would stay "cancelled" while the instance silently appears.
    if !task_is_running(state, task_id).await {
        return Err("任务已取消".to_string());
    }

    // 2. Resolve the actual DSH_HOME: for a dedicated task, create the HOME
    //    record now (path-based reuse keeps it idempotent); otherwise the
    //    caller-provided HOME id must already exist.
    let resolved_home_id = match home_id {
        Some(hid) => hid.clone(),
        None => {
            let path = reserved_home_path
                .ok_or_else(|| "缺少专属 DSH_HOME 路径".to_string())?
                .to_string_lossy()
                .to_string();
            crate::commands::create_home_record(state, name, &path)?.id
        }
    };
    let home_path = {
        let cfg = state.config.lock().unwrap();
        cfg.homes
            .iter()
            .find(|h| h.id == resolved_home_id)
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?
            .path
            .clone()
    };

    // 2.5. Only dedicated (freshly allocated) homes need the baseline `web`
    // profile and `__temp__` template materialized. When the caller selects an
    // existing HOME, that HOME is already established and its profiles should
    // not be touched during instance creation.
    if home_id.is_none() {
        ensure_web_profile_template(app, state, task_id, &home_path, &version_record).await?;
    }

    // 3. Create the instance record.
    let inst = {
        let mut cfg = state.config.lock().unwrap();
        if cfg.instances.iter().any(|i| i.name == name) {
            return Err("同名实例已存在".to_string());
        }
        if !cfg.homes.iter().any(|h| h.id == resolved_home_id) {
            return Err("DSH_HOME 不存在".to_string());
        }
        let inst = DshInstance {
            id: new_id("i"),
            name: name.to_string(),
            version_id: version_record.id.clone(),
            home_id: resolved_home_id,
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,

            port: None,
        };
        cfg.instances.push(inst.clone());
        crate::commands::save_state(state, &cfg)?;
        inst
    };
    Ok(inst.id)
}

/// Ensures the default `web` profile exists in the given DSH_HOME and that a
/// `__temp__` copy (the template later profiles are derived from) is present.
/// If the template is missing, it boots the installed DSH with
/// `--profile web --port <random>`, waits for the web URL (meaning the profile
/// was materialized), terminates it, then copies `profiles/web` to
/// `profiles/__temp__`. The profile for a fresh HOME is created the first time
/// a DSH process runs with that HOME, so this is a one-time cost per HOME.
async fn ensure_web_profile_template(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    home_path: &std::path::Path,
    version: &DshVersion,
) -> Result<(), String> {
    let profiles = crate::profile::profiles_dir(home_path);
    let temp_dir = profiles.join("__temp__");
    if temp_dir.exists() {
        return Ok(());
    }
    let web_dir = profiles.join("web");

    // A HOME that has run DSH before already has a materialized `web` profile
    // (e.g. the default ~/.dsh of a live instance). Booting DSH again would
    // bind the port pinned in that profile's cordis.patch.yml — DSH honors
    // the patch over the CLI `--port` — and crash with EADDRINUSE while the
    // owning instance is running. Copying is all the template needs.
    let web_populated = web_dir.is_dir()
        && std::fs::read_dir(&web_dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
    if web_populated {
        push_task_log(app, state, task_id, "web profile 已存在，直接复制为 __temp__ 模板").await;
        copy_dir(&web_dir, &temp_dir).map_err(|e| format!("复制 __temp__ profile 失败: {e}"))?;
        crate::profile::scrub_profile_port_pin(&temp_dir);
        push_task_log(app, state, task_id, "web profile 模板 __temp__ 已创建").await;
        return Ok(());
    }

    let bin = crate::launch::version_bin(&version.dir);
    if !crate::launch::version_bin_ready(&version.dir) {
        return Err(format!(
            "版本 {} 安装不完整（缺少 {}）",
            version.version,
            bin.display()
        ));
    }

    let port = 20000 + rand_port_offset();
    let msg = format!("正在初始化 web profile（端口 {port}）…");
    push_task_log(app, state, task_id, &msg).await;

    // The launch spec module owns this argv: the template boot is the ONE
    // DSH invocation allowed to pass --host (a throwaway bind whose host is
    // the launcher's own decision).
    let node = crate::runtime::node_for_spawn_checked(state).await?;
    let preserve_symlinks = state.config.lock().unwrap().settings.preserve_symlinks;
    let mut child = crate::launch::template_boot_command(
        &node,
        &version.dir,
        home_path,
        port,
        preserve_symlinks,
    )
    .map_err(|e| format!("启动 DSH 生成 profile 失败: {e}"))?
        .spawn()
        .map_err(|e| format!("启动 DSH 生成 profile 失败: {e}"))?;

    // Drain stderr concurrently: DSH reports boot failures (missing native
    // addons, plugin load errors) on stderr, and an undrained pipe could also
    // fill up and stall the child. Keep the tail for the failure message.
    let stderr_tail = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut stderr_reader = None;
    if let Some(err) = child.stderr.take() {
        let tail = stderr_tail.clone();
        let app2 = app.clone();
        let tid = task_id.to_string();
        stderr_reader = Some(tauri::async_runtime::spawn(async move {
            let state = app2.state::<AppState>();
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let l = l.trim_end_matches(['\r', '\n']).trim().to_string();
                if l.is_empty() {
                    continue;
                }
                {
                    let mut guard = tail.lock().unwrap();
                    if guard.len() >= 200 {
                        guard.remove(0);
                    }
                    guard.push(l.clone());
                }
                push_task_log(&app2, &state, &tid, &l).await;
            }
        }));
    }

    // Wait for the web URL to appear (profile has been created), then stop it.
    let mut timer = tokio::time::interval(std::time::Duration::from_millis(300));
    let mut attempts = 0;
    let mut ready = false;
    if let Some(out) = child.stdout.take() {
        let mut reader = BufReader::new(out).lines();
        loop {
            tokio::select! {
                line = reader.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            let l = l.trim().to_string();
                            if !l.is_empty() {
                                push_task_log(app, state, task_id, &l).await;
                            }
                            if l.contains("dsh web: http") {
                                ready = true;
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                _ = timer.tick() => {
                    attempts += 1;
                    if attempts > 200 { break; } // ~60s safety cap
                }
            }
        }
    }

    // Stop DSH, then give the stderr reader a moment to flush whatever a
    // crash printed (the child is gone, so its pipes hit EOF).
    child.kill().await.ok();
    let _ = child.wait().await;
    if let Some(reader) = stderr_reader.take() {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(1000), reader).await;
    }

    if !ready {
        let summary = {
            let tail = stderr_tail.lock().unwrap();
            let errors: Vec<String> = tail
                .iter()
                .filter(|l| l.to_lowercase().contains("error"))
                .cloned()
                .collect();
            let source: &[String] = if errors.is_empty() { &tail } else { &errors };
            source
                .iter()
                .rev()
                .take(2)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        };
        if summary.is_empty() {
            return Err("生成 web profile 超时或失败".to_string());
        }
        return Err(format!("生成 web profile 超时或失败：{summary}"));
    }

    // Copy profiles/web → profiles/__temp__.
    if !web_dir.exists() {
        return Err("web profile 目录未生成".to_string());
    }
    copy_dir(&web_dir, &temp_dir).map_err(|e| format!("复制 __temp__ profile 失败: {e}"))?;
    crate::profile::scrub_profile_port_pin(&temp_dir);
    push_task_log(app, state, task_id, "web profile 模板 __temp__ 已创建").await;
    Ok(())
}

/// Simple deterministic-ish port offset so multiple homes don't collide often.
fn rand_port_offset() -> u16 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 30000) as u16
}

pub(crate) async fn push_task_log(app: &AppHandle, state: &State<'_, AppState>, task_id: &str, line: &str) {
    let mut tasks = state.tasks.lock().await;
    if let Some(task) = tasks.get_mut(task_id) {
        // Cap the retained log (mirrors stream_pipe's MAX_LOG_LINES).
        if task.logs.len() >= MAX_LOG_LINES {
            task.logs.remove(0);
        }
        task.logs.push(line.to_string());
    }
    emit_log(app, task_id, line);
}

/// Recursively copies a directory tree (files only, directories preserved).
fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Runs `pnpm install --loglevel=http` for the given version, streaming every
/// output line into the task log (and as events). The pnpm content store is
/// placed under the app data dir (`.pnpm-store`) so versions are installed
/// into the launcher's own storage. Returns the new version record on success.
async fn install_version_streamed(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    version: &str,
) -> Result<DshVersion, String> {
    // Alpha builds ship only as GitHub `dsh-v*` tags, never to npm; route
    // those to the from-source pipeline before touching the version dir.
    if !npm_has_version(version).await {
        push_task_log(
            app,
            state,
            task_id,
            &format!("@deepseek-ai/dsh@{version} 未发布到 npm，检查 GitHub 标签 dsh-v{version}…"),
        )
        .await;
        if !github_tag_exists(version).await? {
            return Err(format!(
                "版本 {version} 既未发布到 npm，也不存在 GitHub 标签 dsh-v{version}"
            ));
        }
        return install_version_from_repo(app, state, task_id, version).await;
    }

    let dir = state.data_dir.join("versions").join(version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建版本目录失败: {e}"))?;
    let store_dir = crate::toolchain::store_dir(&state.data_dir);

    // pnpm (>=10) ignores dependency build scripts by default, which would
    // skip native modules like node-pty / koffi. A workspace manifest inside
    // the install dir opts back into running all build scripts; on pnpm 11
    // this also carries the `allowBuilds` section (see
    // crate::plugins::ensure_build_scripts_allowed).
    crate::plugins::ensure_build_scripts_allowed(&dir)?;

    // Make sure a pnpm executable is available before installing: use the
    // system one if present, otherwise bootstrap the latest pnpm into the
    // launcher's data dir via npm.
    let pnpm_prog = crate::toolchain::ensure_pnpm_for_task(app, state, task_id).await?;

    let install_result = install_via_pnpm(app, state, task_id, &dir, &store_dir, &pnpm_prog, version).await;
    if install_result.is_err() {
        // A half-installed tree (plus the allowBuilds workspace manifest this
        // path writes) makes a retry's semantics unclear and wastes disk —
        // clear it so the next attempt starts clean. This version is not
        // registered, so nothing can reference the directory yet.
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            crate::log_warn!("清理失败的版本目录 {} 失败: {e}", dir.display());
        } else {
            push_task_log(app, state, task_id, "已清理未完成的版本目录").await;
        }
    }
    install_result?;

    register_version(state, version, dir)
}

/// The pnpm install + native-build completion phase of an npm version
/// install. Split out of `install_version_streamed` so its failure cleanup
/// covers every error path uniformly.
async fn install_via_pnpm(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    dir: &std::path::Path,
    store_dir: &std::path::Path,
    pnpm_prog: &std::path::Path,
    version: &str,
) -> Result<(), String> {
    // Attempt plan: a failed install retries automatically (transient network
    // errors are common on a ~600-package tree). Before the final attempt the
    // pnpm content store and the version dir are wiped: the store is a pure
    // cache, and one interrupted download can leave corrupted entries in it
    // that fail EVERY later install with integrity errors — a clean slate is
    // the only reliable recovery for that state.
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt: u32 = 1;
    loop {
        let mut cmd = tokio::process::Command::new(pnpm_prog);
        crate::process::hide_console(&mut cmd);
        // Network robustness: the default fetch timeout (60s) and retries (2)
        // are too tight for large native binaries (e.g. sharp-win32-x64),
        // which fail with "error (23) ... aborted due to timeout" on flaky
        // connections. These ride along as pnpm_config_* env vars -- the
        // --config.fetch-* flag spelling hangs pnpm 11 (see
        // crate::toolchain::pnpm_network_env).
        cmd.args(["install", "--prefix"])
            .arg(dir)
            .args(crate::toolchain::pnpm_store_flags(store_dir, "http"));
        crate::toolchain::apply_pnpm_network_env(&mut cmd);
        cmd.arg(format!("@deepseek-ai/dsh@{version}"));
        // No TTY under the launcher: keep pnpm non-interactive so a modules
        // purge (store relink) never aborts with
        // ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY.
        cmd.env("CI", "true");

        match run_streamed_command(app, state, task_id, cmd, "pnpm install").await {
            Ok(()) => break,
            Err(e) => {
                // pnpm 11 blocks dependency build scripts unless every package
                // with an install script is listed under `allowBuilds`. On the
                // first attempt it writes a `set this to true or false`
                // placeholder into pnpm-workspace.yaml and fails with
                // ERR_PNPM_IGNORED_BUILDS; convert that placeholder to `true`
                // and retry once so native deps (node-pty, koffi, …) build.
                if attempt == 1 && task_log_mentions_ignored_builds(state, task_id) {
                    push_task_log(
                        app,
                        state,
                        task_id,
                        "pnpm 11 拦截了构建脚本，正在批准 allowBuilds 后重试…",
                    )
                    .await;
                    crate::plugins::ensure_build_scripts_allowed(dir)?;
                    continue;
                }
                if attempt >= MAX_ATTEMPTS {
                    return Err(e);
                }
                if !task_is_running(state, task_id).await {
                    return Err("任务已取消".to_string());
                }
                if attempt + 1 == MAX_ATTEMPTS {
                    push_task_log(
                        app,
                        state,
                        task_id,
                        "清空 pnpm 缓存存储与版本目录后进行最后一次重试…",
                    )
                    .await;
                    if let Err(e) = std::fs::remove_dir_all(store_dir) {
                        crate::log_warn!("清理 pnpm store 失败: {e}");
                    }
                    if let Err(e) = std::fs::remove_dir_all(dir) {
                        crate::log_warn!("清理版本目录失败: {e}");
                    }
                    std::fs::create_dir_all(dir)
                        .map_err(|e| format!("创建版本目录失败: {e}"))?;
                    crate::plugins::ensure_build_scripts_allowed(dir)?;
                } else {
                    push_task_log(
                        app,
                        state,
                        task_id,
                        &format!("安装失败，正在重试（第 {attempt} 次重试）…"),
                    )
                    .await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                attempt += 1;
            }
        }
    }

    ensure_pending_builds(app, state, task_id, dir, pnpm_prog).await
}

/// Runs `pnpm rebuild` when the finished install still lists packages in
/// `pendingBuilds` (build scripts pnpm skipped). A tree in that state is
/// missing compiled native addons (fs-ext since dsh 0.1.3-alpha.2) and DSH
/// fails to boot with an opaque MODULE_NOT_FOUND, so a half-built version is
/// refused instead of registered.
async fn ensure_pending_builds(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    dir: &std::path::Path,
    pnpm_prog: &std::path::Path,
) -> Result<(), String> {
    let pending = pending_builds(dir);
    if pending.is_empty() {
        return Ok(());
    }
    let shown = pending.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    push_task_log(
        app,
        state,
        task_id,
        &format!(
            "检测到 {} 个依赖的构建脚本未执行（{shown}…），运行 pnpm rebuild 补建…",
            pending.len()
        ),
    )
    .await;
    let mut cmd = tokio::process::Command::new(pnpm_prog);
    crate::process::hide_console(&mut cmd);
    cmd.current_dir(dir).arg("rebuild").env("CI", "true");
    run_streamed_command(app, state, task_id, cmd, "pnpm rebuild").await?;
    let still = pending_builds(dir);
    if !still.is_empty() {
        let shown = still.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        return Err(format!(
            "pnpm rebuild 后仍有 {} 个依赖未完成构建: {shown}",
            still.len()
        ));
    }
    push_task_log(app, state, task_id, "依赖构建脚本补建完成").await;
    Ok(())
}

/// Records an installed version in the config (idempotent by version
/// string).
fn register_version(
    state: &State<'_, AppState>,
    version: &str,
    dir: std::path::PathBuf,
) -> Result<DshVersion, String> {
    let record = DshVersion {
        id: new_id("v"),
        version: version.to_string(),
        dir,
    };
    let mut cfg = state.config.lock().unwrap();
    if let Some(existing) = cfg.versions.iter().find(|v| v.version == *version) {
        return Ok(existing.clone());
    }
    cfg.versions.push(record.clone());
    crate::commands::save_state(state, &cfg)?;
    Ok(record)
}

/// Whether `@deepseek-ai/dsh@<version>` exists on the npm registry. Network
/// or npm failures keep the classic npm path so its own error surfaces.
async fn npm_has_version(version: &str) -> bool {
    let mut cmd = tokio::process::Command::new(crate::process::npm());
    crate::process::hide_console(&mut cmd);
    crate::proxy::apply_to_command(&mut cmd);
    cmd.args(["view", &format!("@deepseek-ai/dsh@{version}"), "version"]);
    if let Some(registry) = crate::toolchain::registry_mirror() {
        cmd.args(["--registry", &registry]);
    }
    // An unknown version exits 0 with empty output.
    match cmd.output().await {
        Ok(out) => out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        Err(_) => true,
    }
}

/// Whether the upstream repo carries the `dsh-v<version>` tag (GitHub-only
/// alpha builds).
async fn github_tag_exists(version: &str) -> Result<bool, String> {
    let url = crate::plugins::github_api_url(&format!(
        "/repos/{}/git/ref/tags/dsh-v{version}",
        crate::commands::DSH_REPO
    ));
    match crate::plugins::fetch_json(&url, 256 * 1024).await {
        Ok(_) => Ok(true),
        Err(e) if e.contains("HTTP 404") => Ok(false),
        // Anonymous GitHub API quota is small; a 403/429 here is rate
        // limiting, and surfacing that plainly beats a cryptic HTTP error.
        Err(e) if e.contains("HTTP 403") || e.contains("HTTP 429") => Err(format!(
            "GitHub API 请求被限流，无法确认版本 {version} 是否存在；请稍后重试{e}"
        )),
        Err(e) => Err(e),
    }
}

/// Installs a GitHub-only version (a `dsh-v<ver>` tag never published to
/// npm) from source, following the upstream README "Run from source" flow:
/// clone the tag → `pnpm install` → `pnpm run build`. The version directory
/// IS the checkout, so the CLI entry is `apps/cli/lib/bin.js` (see
/// `crate::process::version_bin`). This takes much longer than an npm
/// install: a full monorepo dependency install plus a full build.
async fn install_version_from_repo(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    version: &str,
) -> Result<DshVersion, String> {
    let dir = state.data_dir.join("versions").join(version);
    let tag = format!("dsh-v{version}");
    let repo_url = format!("https://github.com/{}.git", crate::commands::DSH_REPO);
    let store_dir = crate::toolchain::store_dir(&state.data_dir);

    // 1. Clone the tag. A kept checkout is reused as-is (tags are immutable);
    //    any other leftover directory is a failed attempt and gets cleared.
    if !dir.join("apps").join("cli").join("package.json").exists() {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("清理残留源码目录失败: {e}"))?;
        }
        push_task_log(
            app,
            state,
            task_id,
            &format!("该版本仅发布在 GitHub，正在克隆 {tag} 源码（浅克隆）…"),
        )
        .await;
        let mut cmd = tokio::process::Command::new("git");
        crate::process::hide_console(&mut cmd);
        cmd.args(["clone", "--depth", "1", "--branch", &tag, &repo_url])
            .arg(&dir)
            // Never prompt for credentials on a public repo.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("CI", "true");
        run_streamed_command(app, state, task_id, cmd, "git clone")
            .await
            .map_err(|e| {
                if e.contains("程序") || e.contains("program") || e.contains("not found") {
                    format!("源码构建需要安装 Git：{e}")
                } else {
                    e
                }
            })?;
    } else {
        push_task_log(app, state, task_id, "复用已克隆的源码目录").await;
    }

    let pnpm_prog = crate::toolchain::ensure_pnpm_for_task(app, state, task_id).await?;

    // 2. Dependencies. The checkout manages its own workspace manifest
    // (including build-script policy), so the launcher's allowBuilds
    // workaround does not apply here.
    push_task_log(
        app,
        state,
        task_id,
        "安装依赖（pnpm install --frozen-lockfile），首次可能需要几分钟…",
    )
    .await;
    let mut cmd = tokio::process::Command::new(&pnpm_prog);
    crate::process::hide_console(&mut cmd);
    cmd.current_dir(&dir)
        .args(["install", "--frozen-lockfile"])
        .args(crate::toolchain::pnpm_store_flags(&store_dir, "http"));
    crate::toolchain::apply_pnpm_network_env(&mut cmd);
    cmd.env("CI", "true");
    run_streamed_command(app, state, task_id, cmd, "pnpm install（源码）").await?;
    ensure_pending_builds(app, state, task_id, &dir, &pnpm_prog).await?;

    // 3. Build. Per the upstream README, `pnpm run build` prepares every
    //    repository artifact; `pnpm dsh web` then runs without rebuilding.
    push_task_log(app, state, task_id, "构建（pnpm run build）…").await;
    let mut cmd = tokio::process::Command::new(&pnpm_prog);
    crate::process::hide_console(&mut cmd);
    cmd.current_dir(&dir)
        .args(["run", "build"])
        .env("CI", "true");
    run_streamed_command(app, state, task_id, cmd, "pnpm run build").await?;

    // 4. Verify and register.
    if !crate::launch::version_bin_ready(&dir) {
        return Err(format!(
            "构建完成后未找到 CLI 入口 {}，请查看任务日志中的构建输出",
            crate::launch::version_bin(&dir).display()
        ));
    }
    register_version(state, version, dir)
}

/// Packages pnpm recorded under `pendingBuilds` in `node_modules/.modules.yaml`
/// — dependencies whose build scripts did not run. The file is JSON-flavored
/// YAML (pnpm 11 writes `"pendingBuilds": []` inline on one line), so it is
/// deserialized instead of scanned line-wise: the old line scanner kept
/// reading past an emptied inline list and mistook the keys below it
/// (`publicHoistPattern`, `registries`, …) for package names, failing healthy
/// installs. A missing or unparsable file still means "nothing pending".
fn pending_builds(dir: &std::path::Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(dir.join("node_modules").join(".modules.yaml")) else {
        return Vec::new();
    };
    parse_pending_builds(&raw)
}

fn parse_pending_builds(raw: &str) -> Vec<String> {
    #[derive(Default, serde::Deserialize)]
    struct ModulesYaml {
        #[serde(rename = "pendingBuilds", default)]
        pending_builds: Vec<String>,
    }
    serde_yaml::from_str::<ModulesYaml>(raw)
        .map(|m| m.pending_builds)
        .unwrap_or_default()
}

/// Whether the task's streamed log mentions pnpm's ignored-build-scripts
/// failure (ERR_PNPM_IGNORED_BUILDS / "Ignored build scripts").
fn task_log_mentions_ignored_builds(state: &State<'_, AppState>, task_id: &str) -> bool {
    let tasks = state.tasks.try_lock().map(|t| t.clone()).ok();
    tasks
        .and_then(|t| t.get(task_id).map(|t| t.logs.clone()))
        .map(|logs| {
            logs.iter().any(|l| {
                l.contains("ERR_PNPM_IGNORED_BUILDS") || l.contains("Ignored build scripts")
            })
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------

enum StreamPipe {
    Out(tokio::process::ChildStdout),
    Err(tokio::process::ChildStderr),
}

impl tokio::io::AsyncRead for StreamPipe {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamPipe::Out(p) => std::pin::Pin::new(p).poll_read(cx, buf),
            StreamPipe::Err(p) => std::pin::Pin::new(p).poll_read(cx, buf),
        }
    }
}

async fn stream_pipe(app: AppHandle, task_id: String, pipe: StreamPipe) {
    let state = app.state::<AppState>();
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            continue;
        }
        let percent = {
            let mut tasks = state.tasks.lock().await;
            match tasks.get_mut(&task_id) {
                Some(task) if task.state == TaskState::Running => {
                    push_log_locked(task, &line);
                    let pct = (task.percent + 1).min(90);
                    task.percent = pct;
                    // Throttle: emit progress roughly every 20 log lines.
                    if task.logs.len() % 20 == 0 {
                        Some(pct)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
        emit_log(&app, &task_id, &line);
        if let Some(pct) = percent {
            emit_progress(&app, &task_id, TaskState::Running, pct, None, None);
        }
    }
}

pub(crate) fn push_log_locked(task: &mut TaskInfo, line: &str) {
    if task.logs.len() >= MAX_LOG_LINES {
        task.logs.remove(0);
    }
    task.logs.push(line.to_string());
}

pub(crate) fn emit_progress(
    app: &AppHandle,
    id: &str,
    state: TaskState,
    percent: u32,
    message: Option<String>,
    instance_id: Option<String>,
) {
    let _ = app.emit(
        TASK_PROGRESS_EVENT,
        TaskProgress {
            id: id.to_string(),
            state,
            percent,
            message,
            instance_id,
        },
    );
}

fn emit_log(app: &AppHandle, id: &str, line: &str) {
    let _ = app.emit(
        TASK_LOG_EVENT,
        TaskLog {
            id: id.to_string(),
            line: line.to_string(),
        },
    );
}

/// Runs a piped child command as a task: streams stdout/stderr into the task
/// log, exposes the child for cancellation, and nudges the percent upward
/// while it runs. Returns Err when the command fails or is cancelled.
pub(crate) async fn run_streamed_command(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    mut cmd: tokio::process::Command,
    what: &str,
) -> Result<(), String> {
    // Echo the exact command line into the task log: when a child fails
    // silently (pnpm at --loglevel=warn prints nothing for some errors), the
    // invocation itself is the only clue.
    let cmdline = {
        let std_cmd = cmd.as_std();
        let mut s = std_cmd.get_program().to_string_lossy().to_string();
        for arg in std_cmd.get_args() {
            s.push(' ');
            s.push_str(&arg.to_string_lossy());
        }
        s
    };
    push_task_log(app, state, task_id, &format!("$ {cmdline}")).await;
    crate::log_debug!("run_streamed_command[{what}]: {cmdline}");

    // The launcher's proxy must apply to the children it spawns (pnpm, git),
    // not just its own reqwest requests.
    crate::proxy::apply_to_command(&mut cmd);

    // Capture output centrally: callers build bare commands, and a child
    // without piped stdio prints its errors straight into the GUI process's
    // inherited stdout where nobody sees them — the task log then holds
    // nothing but the echoed command line.
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{what} 启动失败: {e}（请确认已安装 Node.js 与 pnpm）"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let shared_child: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(Some(child)));

    {
        let mut tasks = state.tasks.lock().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.child = Some(shared_child.clone());
        }
    }

    let mut pipe_readers = Vec::new();
    for pipe in [stdout.map(StreamPipe::Out), stderr.map(StreamPipe::Err)]
        .into_iter()
        .flatten()
    {
        let app2 = app.clone();
        let tid = task_id.to_string();
        pipe_readers.push(tauri::async_runtime::spawn(async move {
            stream_pipe(app2, tid, pipe).await;
        }));
    }

    // Heartbeat keeps the percent moving while the command is quiet.
    {
        let app2 = app.clone();
        let tid = task_id.to_string();
        let hb_child = shared_child.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                let still_running = {
                    let mut guard = hb_child.lock().await;
                    match guard.as_mut() {
                        Some(c) => matches!(c.try_wait(), Ok(None)),
                        None => false,
                    }
                };
                if !still_running {
                    break;
                }
                let state = app2.state::<AppState>();
                let mut tasks = state.tasks.lock().await;
                let Some(task) = tasks.get_mut(&tid) else {
                    break;
                };
                if task.state != TaskState::Running {
                    break;
                }
                if task.percent < 90 {
                    task.percent = (task.percent + 2).min(90);
                } else if task.percent < 99 {
                    task.percent += 1;
                } else {
                    break;
                }
                let pct = task.percent;
                drop(tasks);
                emit_progress(&app2, &tid, TaskState::Running, pct, None, None);
            }
        });
    }

    let status = loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let mut guard = shared_child.lock().await;
        let Some(child) = guard.as_mut() else {
            return Err("任务已取消".to_string());
        };
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => continue,
            Err(e) => return Err(format!("{what} 等待失败: {e}")),
        }
    };

    // Clear the child handle so a later cancel is a no-op.
    {
        let mut tasks = state.tasks.lock().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.child = None;
        }
    }

    // Drain the pipe readers before judging the outcome: the child has
    // exited but the reader tasks may still be flushing buffered lines into
    // the task log. Reading the log here without waiting races them and
    // loses the very error lines the failure summary is built from.
    for reader in pipe_readers {
        let _ = reader.await;
    }

    if !status.success() {
        let logs = {
            let tasks = state.tasks.lock().await;
            tasks
                .get(task_id)
                .map(|t| t.logs.clone())
                .unwrap_or_default()
        };
        // Prefer a meaningful error line (pnpm error codes, "error:",
        // "aborted", "failed", "timeout") and keep the lines around it —
        // pnpm's remedy ("reinstall your dependencies with pnpm install")
        // sits on the following lines and is exactly what the user needs.
        let is_error_line = |l: &str| {
            let s = l.to_lowercase();
            s.contains("err_pnpm")
                || s.contains("error")
                || s.contains("aborted")
                || s.contains("failed")
                || s.contains("timeout")
        };
        let summary = match logs.iter().rposition(|l| is_error_line(l)) {
            Some(i) => logs[i..]
                .iter()
                .filter(|l| !l.trim().is_empty())
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
            None => {
                let mut tail: Vec<String> = logs
                    .iter()
                    .rev()
                    .filter(|l| !l.trim().is_empty())
                    .take(3)
                    .cloned()
                    .collect();
                tail.reverse();
                tail.join(" | ")
            }
        };
        let summary = if summary.is_empty() {
            format!("{what} 退出码 {status}（子进程未输出任何日志）")
        } else {
            summary
        };
        crate::log_warn!("{what} 失败（{status}）: {summary}");
        return Err(format!("{what} 失败: {summary}"));
    }
    crate::log_debug!("run_streamed_command[{what}]: 完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_version_string_accepts_semver_shapes() {
        assert!(valid_version_string("1.2.3"));
        assert!(valid_version_string("0.1.3-alpha.2"));
        assert!(valid_version_string("0.2.0-rc.8+build.1"));
        assert!(valid_version_string("22.14.0"));
    }

    #[test]
    fn valid_version_string_rejects_path_segments() {
        // Path traversal and separators must never reach a path join.
        assert!(!valid_version_string(""));
        assert!(!valid_version_string(".."));
        assert!(!valid_version_string("../evil"));
        assert!(!valid_version_string("a/b"));
        assert!(!valid_version_string("a\\b"));
        assert!(!valid_version_string("v1.2.3\n"));
        assert!(!valid_version_string("1.2.3 "));
        // GitHub tag names may contain slashes; the installer must refuse them.
        assert!(!valid_version_string("../../evil"));
        // Overlong garbage is rejected too.
        assert!(!valid_version_string(&"1".repeat(65)));
    }

    #[test]
    fn pending_builds_empty_inline_list_is_not_confused_by_following_keys() {
        // Regression: pnpm 11 writes `"pendingBuilds": []` inline; the keys
        // below it (publicHoistPattern, registries, …) are not packages.
        let raw = r#"{
  "pendingBuilds": [],
  "publicHoistPattern": [],
  "registries": {
    "default": "https://registry.npmjs.org/"
  }
}"#;
        assert!(parse_pending_builds(raw).is_empty());
    }

    #[test]
    fn pending_builds_reads_populated_block_list() {
        let raw = "pendingBuilds:\n  - koffi@3.2.1\n  - protobufjs@7.6.6\npublicHoistPattern: []\n";
        assert_eq!(
            parse_pending_builds(raw),
            vec!["koffi@3.2.1".to_string(), "protobufjs@7.6.6".to_string()]
        );
    }

    #[test]
    fn pending_builds_missing_or_garbage_means_nothing_pending() {
        assert!(parse_pending_builds("").is_empty());
        assert!(parse_pending_builds("not yaml: [unclosed").is_empty());
        assert!(parse_pending_builds("{\"other\": 1}").is_empty());
    }

    /// Fresh temp home with a `profiles/<profile>/cordis.patch.yml` holding
    /// `raw`. The caller removes the returned root.
    fn patch_fixture(profile: &str, raw: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dsh-lan-bind-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = root.join("profiles").join(profile);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cordis.patch.yml"), raw).unwrap();
        root
    }

    const MANAGED_BLOCK: &str = "# --- remote-web-ui lan-bind block (managed - do not edit) ---\n\
- id: webserver\n  \
name: '@deepseek-ai/dsh-host-webserver'\n  \
config:\n    host: '0.0.0.0'\n    port: 3080\n    compression: gzip\n\
# --- end remote-web-ui lan-bind block ---\n";

    #[test]
    fn assert_lan_bind_port_rewrites_only_the_managed_port() {
        let home = patch_fixture("web", MANAGED_BLOCK);
        crate::profile::assert_profile_lan_bind_port(&home, "web", 3099);
        let text =
            std::fs::read_to_string(home.join("profiles/web/cordis.patch.yml")).unwrap();
        assert!(text.contains("port: 3099"), "{text}");
        assert!(!text.contains("port: 3080"), "{text}");
        // Host (the LAN toggle's other half) and the rest of the block survive.
        assert!(text.contains("host: '0.0.0.0'"), "{text}");
        assert!(text.contains("compression: gzip"), "{text}");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn assert_lan_bind_port_leaves_unmanaged_profiles_alone() {
        // No managed block: the CLI --port already wins, so nothing is written
        // and a hand-authored webserver row keeps its pin.
        let home = patch_fixture("web", "[]\n\n- id: webserver\n  config:\n    port: 3080\n");
        crate::profile::assert_profile_lan_bind_port(&home, "web", 3099);
        let text =
            std::fs::read_to_string(home.join("profiles/web/cordis.patch.yml")).unwrap();
        assert_eq!(text, "[]\n\n- id: webserver\n  config:\n    port: 3080\n");

        // A missing patch file is a no-op, not an error.
        crate::profile::assert_profile_lan_bind_port(&home, "absent", 3099);
        assert!(!home.join("profiles/absent").exists());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn assert_lan_bind_port_refuses_path_traversal_profile() {
        let home = patch_fixture("web", MANAGED_BLOCK);
        // An escaping name must not write outside profiles/web.
        crate::profile::assert_profile_lan_bind_port(&home, "../web", 3099);
        crate::profile::assert_profile_lan_bind_port(&home, "..", 3099);
        crate::profile::assert_profile_lan_bind_port(&home, "/tmp/evil", 3099);
        let text =
            std::fs::read_to_string(home.join("profiles/web/cordis.patch.yml")).unwrap();
        assert!(text.contains("port: 3080"), "{text}");
        std::fs::remove_dir_all(&home).ok();
    }
}
