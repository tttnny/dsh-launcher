use crate::config::{new_id, DshInstance, DshVersion, Host};
use crate::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

pub const TASK_PROGRESS_EVENT: &str = "task://progress";
pub const TASK_LOG_EVENT: &str = "task://log";

const MAX_LOG_LINES: usize = 1000;

/// How long one read of a remote install's log parks on that machine waiting
/// for output. The wait happens there, so a quiet install costs one ssh call per
/// this many seconds instead of one per second, while new output still reaches
/// the page within about a second.
const REMOTE_TAIL_WAIT_SECS: u64 = 10;

/// How long a cancelled remote install may take to actually disappear before
/// the launcher says so instead of pretending it stopped, and how often it
/// asks meanwhile.
const REMOTE_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const REMOTE_STOP_TICK: std::time::Duration = std::time::Duration::from_millis(200);

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
    pub kind: String, // "install-version"
    pub label: String,
    pub version: String,
    /**
     * The machine this task works on. Shown on the row, and half of the key the
     * page tracks progress under: two hosts can install the same version at the
     * same time, and their progress must not overwrite each other's.
     */
    pub host_id: String,
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
    /// The remote command in flight, when this task installs on another machine.
    /// Cancelling needs it to stop the **process tree** up there, not just the
    /// progress bar (issue #9 风险). Not serialized.
    #[serde(skip)]
    pub remote: Option<RemoteRun>,
}

/// One detached command the launcher started on a remote host and is following:
/// where it runs, and the pid that leads its process group.
#[derive(Clone, Debug)]
pub struct RemoteRun {
    pub host: crate::ssh::RemoteHost,
    pub pid: i32,
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

/// Enqueues a background task that installs the given DSH version **on one
/// host** (if not installed there yet), auto-creates its dedicated HOME, and
/// registers the 1:1 instance. `host_id` absent means 本机.
#[tauri::command(rename_all = "snake_case")]
pub async fn start_install_version_task(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
    host_id: Option<String>,
) -> Result<String, String> {
    let v = version.trim().to_string();
    start_create_instance_task(app, state, v.clone(), v, None, true, host_id).await
}

/// Enqueues a background task that installs the given DSH version (if not
/// installed yet) and then creates the instance. Returns the task id.
/// Internal: only reachable through `start_install_version_task`. Installing a
/// version acquires its directory; the dedicated HOME and the 1:1 instance are
/// recorded by the task body only after the install succeeds.
async fn start_create_instance_task(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    version: String,
    home_id: Option<String>,
    dedicated: bool,
    host_id: Option<String>,
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
    let host = {
        let cfg = state.config.lock().unwrap();
        crate::config::resolve_host(&cfg, host_id.as_deref())?.clone()
    };

    // Dedicated HOME: reserve the path now (placeholder) but do NOT create the
    // HOME record yet — it is created only once the instance is actually made,
    // so a failed/cancelled task leaves no orphan HOME behind.
    let reserved_home_path: Option<std::path::PathBuf> = if dedicated {
        let path = host.dedicated_home_path(&state.data_dir, &name)?;
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
    // can never both pass the checks (TOCTOU). A running install-version task
    // for the same version **on the same host** must also be rejected: the
    // version record is only registered when its install finishes, so two
    // installs of the same version would otherwise run `pnpm install --prefix`
    // into the same directory concurrently and corrupt the tree. Two hosts are
    // two directories, so they may install the same version side by side.
    let task_id = {
        let mut tasks = state.tasks.lock().await;
        for task in tasks.values() {
            if task.state == TaskState::Running {
                if task.instance_name.as_deref() == Some(name.as_str()) {
                    return Err("同名实例的下载任务已在进行中".to_string());
                }
                if task.host_id != host.id {
                    continue;
                }
                if task.kind == "install-version" && task.version == version {
                    return Err(format!("版本 {version} 的下载任务已在进行中"));
                }
                if let (Some(a), Some(b)) = (&task.reserved_home_path, &reserved_home_path) {
                    if host.paths_equal(a, b) {
                        return Err("该专属 DSH_HOME 已被其他下载任务占用".to_string());
                    }
                }
            }
        }
        let task = TaskInfo {
            id: new_id("t"),
            kind: "install-version".to_string(),
            label: task_label,
            version: version.clone(),
            host_id: host.id.clone(),
            state: TaskState::Running,
            percent: 0,
            created_at: now_millis(),
            message: None,
            instance_id: None,
            instance_name: Some(name.clone()),
            reserved_home_path,
            logs: Vec::new(),
            child: None,
            remote: None,
        };
        let id = task.id.clone();
        tasks.insert(id.clone(), task);
        id
    };
    emit_progress(&app, &task_id, TaskState::Running, 0, None, None);

    let worker_app = app.clone();
    let worker_task_id = task_id.clone();
    let worker_host = host.clone();
    let worker_name = name.clone();
    let worker_version = version.clone();
    let worker_home_id = home_id.clone();
    tauri::async_runtime::spawn(async move {
        let state = worker_app.state::<AppState>();
        run_create_instance_task(
            &worker_app,
            &state,
            InstallRequest {
                task_id: &worker_task_id,
                name: &worker_name,
                version: &worker_version,
                home_id: &worker_home_id,
                reserved_home_path: None,
                host: &worker_host,
            },
        )
        .await;
    });

    Ok(task_id)
}

// ---------------------------------------------------------------------------
// Task runner — completion bookkeeping shared by every background task kind
// (install-version is the only one today). A task body runs to `Ok(payload)` /
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
    let (child, remote) = {
        let tasks = state.tasks.lock().await;
        match tasks.get(&id) {
            Some(task) => (task.child.clone(), task.remote.clone()),
            None => (None, None),
        }
    };

    // A remote install is stopped **on that machine** before the launcher says
    // 「已取消」: a pnpm left running holds the store's lock and keeps writing
    // into a directory the launcher is about to clean up (issue #9 风险). When
    // that fails, the task is left running and the user is told why, because the
    // install up there is in fact still running.
    if let Some(run) = &remote {
        stop_remote_run(&state, run).await?;
    }

    {
        let mut tasks = state.tasks.lock().await;
        if let Some(task) = tasks.get_mut(&id) {
            // An install that finished while the stop was in flight keeps its
            // own outcome: it is not cancelled, it is done.
            if task.state == TaskState::Running {
                task.state = TaskState::Cancelled;
                task.message = Some("已取消".to_string());
            }
            task.remote = None;
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

/// What one install task is about: the name, the version, the DSH_HOME it may
/// bind, and the machine all of it lives on. One value instead of five
/// arguments travelling together through the worker.
struct InstallRequest<'a> {
    task_id: &'a str,
    name: &'a str,
    version: &'a str,
    /// The HOME the caller chose, when it chose one.
    home_id: &'a Option<String>,
    /// The dedicated HOME path reserved while the task runs.
    reserved_home_path: Option<&'a Path>,
    host: &'a Host,
}

async fn run_create_instance_task(
    app: &AppHandle,
    state: &State<'_, AppState>,
    request: InstallRequest<'_>,
) {
    // The dedicated HOME path is read from the task's reservation; only then
    // is the actual HOME record created (inside do_create_instance).
    let reserved = {
        let tasks = state.tasks.lock().await;
        tasks
            .get(request.task_id)
            .and_then(|t| t.reserved_home_path.clone())
    };
    let request = InstallRequest {
        reserved_home_path: reserved.as_deref(),
        ..request
    };
    let task_id = request.task_id;
    let result = do_create_instance(app, state, &request).await;

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
    request: &InstallRequest<'_>,
) -> Result<String, String> {
    let InstallRequest {
        task_id,
        name,
        version,
        home_id,
        reserved_home_path,
        host,
    } = *request;
    // 1. Install the version if that host does not have it yet.
    let version_record = {
        let cfg = state.config.lock().unwrap();
        cfg.versions
            .iter()
            .find(|v| v.version == version && v.host_id == host.id)
            .cloned()
    };
    let version_record = match version_record {
        Some(v) => v,
        None => {
            if !task_is_running(state, task_id).await {
                return Err("任务已取消".to_string());
            }
            install_version_streamed(app, state, task_id, host, version).await?
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
            create_dedicated_home(state, host, name, &path).await?
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
    //
    // Only 本机 bootstraps it here: materializing the template means starting
    // DSH against that HOME and stopping it again, and on another machine that
    // is an instance launch with its own lifecycle (a later ticket). A remote
    // dedicated HOME gets its `profiles/web` the first time DSH runs there,
    // which is where the local template is copied from anyway.
    if home_id.is_none() && host.builtin {
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
            host_id: version_record.host_id.clone(),
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

/// Records the dedicated DSH_HOME an install's instance binds to, making the
/// directory where it lives first: this Mac's own filesystem for 本机, or over
/// the transport for a remote host.
///
/// The record itself is local either way (ADR-0006): only the directory is on
/// the machine that owns it, and a path that already has a record on that same
/// host is that HOME rather than a second one.
async fn create_dedicated_home(
    state: &State<'_, AppState>,
    host: &Host,
    name: &str,
    path: &str,
) -> Result<String, String> {
    if host.builtin {
        return Ok(crate::commands::create_home_record_in(state, name, path, None)?.id);
    }
    let target = crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;
    crate::remote::ensure_dir(&*state.host_transport, &target, path)
        .await
        .map_err(|e| format!("在那台机器上创建专属 DSH_HOME 目录失败：{e}"))?;

    let path_buf = PathBuf::from(path);
    let mut cfg = state.config.lock().unwrap();
    if let Some(existing) = cfg
        .homes
        .iter()
        .find(|h| h.host_id == host.id && host.paths_equal(&h.path, &path_buf))
    {
        return Ok(existing.id.clone());
    }
    let home = crate::config::DshHome {
        id: new_id("h"),
        name: name.to_string(),
        path: path_buf,
        host_id: host.id.clone(),
    };
    cfg.homes.push(home.clone());
    crate::commands::save_state(state, &cfg)?;
    Ok(home.id)
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
    let mut child = crate::launch::template_boot_command(&node, &version.dir, home_path, port)
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
///
/// 本机 installs into the launcher's own data directory; `host` names the
/// machine the version belongs to, and another machine has its own install
/// ([`install_version_remote`]) — with the same shape, and not one local
/// filesystem call in it.
async fn install_version_streamed(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    host: &Host,
    version: &str,
) -> Result<DshVersion, String> {
    if !host.builtin {
        return install_version_remote(app, state, task_id, host, version).await;
    }

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

    let layout = install_layout(host, &state.data_dir, version)?;
    let dir = layout.dir.clone();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建版本目录失败: {e}"))?;

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

    let install_result = install_via_pnpm(app, state, task_id, &dir, &layout.store_dir, &pnpm_prog, version).await;
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

    register_version(state, version, dir, &host.id)
}

// ---------------------------------------------------------------------------
// Installing on a remote host
// ---------------------------------------------------------------------------

/// Where a version install runs and what it runs with, on **one** host.
///
/// 本机's answer is the three paths the installer has always used — the
/// launcher's own data directory with `versions/` and `.pnpm-store` under it,
/// and the launcher's own registry mirror — so the local install is what it was.
/// Another machine's are that machine's, taken from its record: the data
/// directory the user configured for it, the registry its versions come from,
/// and the environment variables everything run there gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstallLayout {
    /// The host's `versions/` directory — where a clone lands.
    pub versions_dir: PathBuf,
    /// The one version's own directory.
    pub dir: PathBuf,
    /// The host's pnpm content store.
    pub store_dir: PathBuf,
    /// The whole of the host's data directory, which is the boundary every
    /// cleanup on it stays inside.
    pub data_dir: PathBuf,
    pub registry: Option<String>,
    pub extra_env: Vec<(String, String)>,
}

pub(crate) fn install_layout(
    host: &Host,
    launcher_data_dir: &Path,
    version: &str,
) -> Result<InstallLayout, String> {
    let data_dir = host.data_dir(launcher_data_dir)?;
    let versions_dir = data_dir.join("versions");
    let dir = versions_dir.join(version);
    let store_dir = crate::toolchain::store_dir(&data_dir);
    let registry = if host.builtin {
        crate::toolchain::registry_mirror()
    } else {
        host.registry
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
    };
    Ok(InstallLayout {
        versions_dir,
        dir,
        store_dir,
        data_dir,
        registry,
        extra_env: if host.builtin {
            // 本机's network exit is the launcher's own proxy, applied to the
            // commands it spawns; a remote host's is whatever its record says.
            Vec::new()
        } else {
            host.extra_env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        },
    })
}

/// Runs one version install **on a remote host** and returns the new version
/// record.
///
/// The shape is the local install's — decide npm or source, install, register,
/// the same retry and cleanup rules — with every path taken from that host's
/// data directory and every command run there. Nothing in here touches this
/// Mac's filesystem: the version directory, the pnpm store and the logs are all
/// on the machine that owns them (ADR-0006).
async fn install_version_remote(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    host: &Host,
    version: &str,
) -> Result<DshVersion, String> {
    let layout = install_layout(host, &state.data_dir, version)?;
    let target = crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;

    // The pnpm that drives the install is the one that machine's own probe
    // resolved: the same binary the environment page reports, or a failure that
    // names the command installing one there.
    let pnpm = crate::toolchain::ensure_pnpm_for_task_on(app, state, task_id, Some(&host.id))
        .await?;

    // Whether that machine's npm knows the version decides between installing
    // it there and building it from source there — the same fork the local
    // installer takes, asked of the machine that will do the work.
    let package = format!("@deepseek-ai/dsh@{version}");
    let on_npm = crate::remote::npm_has_version(
        &*state.host_transport,
        &target,
        &package,
        layout.registry.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;

    let result = if on_npm {
        install_remote_from_npm(app, state, task_id, &target, &layout, &pnpm, version).await
    } else {
        push_task_log(
            app,
            state,
            task_id,
            &format!(
                "@deepseek-ai/dsh@{version} 不在那台机器的 npm 上，检查 GitHub 标签 dsh-v{version} 并从源码构建…"
            ),
        )
        .await;
        install_remote_from_source(app, state, task_id, &target, &layout, &pnpm, version).await
    };

    if result.is_err() && on_npm {
        // The local rule, applied where the directory actually is: an
        // unfinished npm tree makes a retry's semantics unclear, and the
        // version is not registered yet so nothing can reference it.
        remove_remote_dir(state, &target, &layout.dir, &layout.data_dir).await;
        push_task_log(app, state, task_id, "已清理未完成的版本目录").await;
    }
    result?;

    register_version(state, version, layout.dir, &host.id)
}

/// The pnpm install + native-build phase on a remote host, with the local
/// retry rules: a failed attempt is retried, and before the last one that
/// machine's pnpm store and version directory are cleared — the store is a pure
/// cache, and one interrupted download can leave entries that fail **every**
/// later install with integrity errors (see [`install_via_pnpm`]).
async fn install_remote_from_npm(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    target: &crate::ssh::RemoteHost,
    layout: &InstallLayout,
    pnpm: &Path,
    version: &str,
) -> Result<(), String> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt: u32 = 1;
    loop {
        // The workspace manifest that opts pnpm back into dependency build
        // scripts is written before the install, exactly as it is locally —
        // over the transport, because it lives in the version directory there.
        ensure_remote_build_scripts_allowed(state, target, &layout.dir).await?;

        let argv = remote_npm_install_argv(pnpm, layout, version);
        let log_path = remote_log_path(&layout.data_dir, task_id, &format!("npm-{attempt}"));

        let code = match run_remote_streamed(
            app,
            state,
            task_id,
            target,
            &log_path,
            argv,
            "pnpm install",
        )
        .await
        {
            Ok(code) => code,
            // A transport failure is not the install's own failure: retrying a
            // machine the launcher cannot reach would only repeat it.
            Err(e) => return Err(e),
        };
        if code == 0 {
            break;
        }

        let logs = task_logs(state, task_id).await;
        // pnpm 11 blocks dependency build scripts unless every package with an
        // install script is listed under `allowBuilds`. On the first attempt it
        // writes a `set this to true or false` placeholder into
        // pnpm-workspace.yaml and fails with ERR_PNPM_IGNORED_BUILDS; the
        // rewrite at the top of the next round converts that placeholder to
        // `true`, so one retry has native deps (node-pty, koffi, …) build.
        if attempt == 1 && mentions_ignored_builds(&logs) {
            push_task_log(
                app,
                state,
                task_id,
                "pnpm 11 拦截了构建脚本，正在批准 allowBuilds 后重试…",
            )
            .await;
            continue;
        }
        if attempt >= MAX_ATTEMPTS {
            return Err(remote_install_failure(&logs, Some(code), &log_path));
        }
        if !task_is_running(state, task_id).await {
            return Err("任务已取消".to_string());
        }
        if attempt + 1 == MAX_ATTEMPTS {
            push_task_log(
                app,
                state,
                task_id,
                "清空那台机器上的 pnpm 缓存存储与版本目录后进行最后一次重试…",
            )
            .await;
            remove_remote_dir(state, target, &layout.store_dir, &layout.data_dir).await;
            remove_remote_dir(state, target, &layout.dir, &layout.data_dir).await;
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

    ensure_remote_pending_builds(app, state, task_id, target, layout, pnpm).await
}

/// Installs a GitHub-only version (a `dsh-v<ver>` tag never published to npm)
/// from source **on a remote host**, following the upstream README "Run from
/// source" flow there: clone the tag → `pnpm install` → `pnpm run build`. The
/// version directory IS the checkout, so the CLI entry is
/// `apps/cli/lib/bin.js`, the layout [`crate::launch::version_bin`] uses for a
/// checkout.
async fn install_remote_from_source(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    target: &crate::ssh::RemoteHost,
    layout: &InstallLayout,
    pnpm: &Path,
    version: &str,
) -> Result<(), String> {
    let tag = format!("dsh-v{version}");

    // 1. Clone the tag. A kept checkout is reused as-is (tags are immutable);
    //    any other leftover directory is a failed attempt and gets cleared.
    let checkout_manifest = layout.dir.join("apps").join("cli").join("package.json");
    let reused = state
        .host_transport
        .read_file(target, &checkout_manifest.to_string_lossy())
        .await
        .is_ok();
    if reused {
        push_task_log(app, state, task_id, "复用已克隆的源码目录").await;
    } else {
        remove_remote_dir(state, target, &layout.dir, &layout.data_dir).await;
        push_task_log(
            app,
            state,
            task_id,
            &format!("该版本仅发布在 GitHub，正在那台机器上克隆 {tag} 源码（浅克隆）…"),
        )
        .await;
        let argv = crate::remote::measured_argv(
            &remote_git_argv(
                layout,
                &[
                    "clone".to_string(),
                    "--depth".to_string(),
                    "1".to_string(),
                    "--branch".to_string(),
                    tag.clone(),
                    format!("https://github.com/{}.git", crate::commands::DSH_REPO),
                    layout.dir.to_string_lossy().to_string(),
                ],
            ),
            &layout.versions_dir.to_string_lossy(),
        );
        let log_path = remote_log_path(&layout.data_dir, task_id, "clone");
        let code =
            run_remote_streamed(app, state, task_id, target, &log_path, argv, "git clone").await?;
        if code != 0 {
            let logs = task_logs(state, task_id).await;
            return Err(remote_install_failure(&logs, Some(code), &log_path));
        }
    }

    // 2. Dependencies. The checkout manages its own workspace manifest
    //    (including build-script policy), so the launcher's allowBuilds
    //    workaround does not apply here.
    push_task_log(
        app,
        state,
        task_id,
        "安装依赖（pnpm install --frozen-lockfile），首次可能需要几分钟…",
    )
    .await;
    let mut deps = vec!["install".to_string(), "--frozen-lockfile".to_string()];
    deps.extend(crate::toolchain::pnpm_store_flags_with(
        &layout.store_dir,
        "http",
        layout.registry.as_deref(),
    ));
    let argv =
        crate::remote::measured_argv(&remote_pnpm_argv(pnpm, layout, &deps), &layout.dir.to_string_lossy());
    let log_path = remote_log_path(&layout.data_dir, task_id, "deps");
    let code = run_remote_streamed(app, state, task_id, target, &log_path, argv, "pnpm install（源码）").await?;
    if code != 0 {
        let logs = task_logs(state, task_id).await;
        return Err(remote_install_failure(&logs, Some(code), &log_path));
    }
    ensure_remote_pending_builds(app, state, task_id, target, layout, pnpm).await?;

    // 3. Build. Per the upstream README, `pnpm run build` prepares every
    //    repository artifact; `pnpm dsh web` then runs without rebuilding.
    push_task_log(app, state, task_id, "构建（pnpm run build）…").await;
    let argv = crate::remote::measured_argv(
        &remote_pnpm_argv(pnpm, layout, &["run".to_string(), "build".to_string()]),
        &layout.dir.to_string_lossy(),
    );
    let log_path = remote_log_path(&layout.data_dir, task_id, "build");
    let code = run_remote_streamed(app, state, task_id, target, &log_path, argv, "pnpm run build").await?;
    if code != 0 {
        let logs = task_logs(state, task_id).await;
        return Err(remote_install_failure(&logs, Some(code), &log_path));
    }

    // 4. Verify: the same two layouts the local flow checks, read on the
    //    machine that produced them.
    let versions_dir = layout.versions_dir.to_string_lossy().to_string();
    let installed = crate::remote::installed_versions(&*state.host_transport, target, &versions_dir)
        .await
        .map_err(|e| e.to_string())?;
    if !installed
        .iter()
        .any(|v| v.name == version && v.bin_ready)
    {
        return Err(format!(
            "构建完成后未找到 CLI 入口 {}/apps/cli/lib/bin.js，请查看任务日志中的构建输出",
            layout.dir.display()
        ));
    }
    Ok(())
}

/// The remote twin of [`ensure_pending_builds`]: a tree whose `pendingBuilds`
/// list is not empty is missing compiled native addons (fs-ext since dsh
/// 0.1.3-alpha.2) and DSH fails to boot with an opaque MODULE_NOT_FOUND, so
/// `pnpm rebuild` runs there and a tree still incomplete afterwards is refused
/// instead of registered.
async fn ensure_remote_pending_builds(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    target: &crate::ssh::RemoteHost,
    layout: &InstallLayout,
    pnpm: &Path,
) -> Result<(), String> {
    let modules = layout.dir.join("node_modules").join(".modules.yaml");
    let path = modules.to_string_lossy().to_string();
    let mut pending = remote_pending_builds(state, target, &path).await;
    if pending.is_empty() {
        return Ok(());
    }
    let shown = pending.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    push_task_log(
        app,
        state,
        task_id,
        &format!(
            "检测到 {} 个依赖的构建脚本未执行（{shown}…），在那台机器上运行 pnpm rebuild 补建…",
            pending.len()
        ),
    )
    .await;
    let argv = crate::remote::measured_argv(
        &remote_pnpm_argv(pnpm, layout, &["rebuild".to_string()]),
        &layout.dir.to_string_lossy(),
    );
    let log_path = remote_log_path(&layout.data_dir, task_id, "rebuild");
    let code = run_remote_streamed(app, state, task_id, target, &log_path, argv, "pnpm rebuild").await?;
    if code != 0 {
        let logs = task_logs(state, task_id).await;
        return Err(remote_install_failure(&logs, Some(code), &log_path));
    }
    pending = remote_pending_builds(state, target, &path).await;
    if !pending.is_empty() {
        let shown = pending.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        return Err(format!(
            "pnpm rebuild 后仍有 {} 个依赖未完成构建: {shown}",
            pending.len()
        ));
    }
    push_task_log(app, state, task_id, "依赖构建脚本补建完成").await;
    Ok(())
}

/// A missing `.modules.yaml` means "nothing pending", exactly as it does
/// locally: a tree without it has no build scripts pnpm passed over.
async fn remote_pending_builds(
    state: &State<'_, AppState>,
    target: &crate::ssh::RemoteHost,
    modules_yaml: &str,
) -> Vec<String> {
    match state.host_transport.read_file(target, modules_yaml).await {
        Ok(bytes) => parse_pending_builds(&String::from_utf8_lossy(&bytes)),
        Err(_) => Vec::new(),
    }
}

/// The argv for one pnpm command on a remote host: that host's own environment
/// variables and the launcher's pnpm policy (store, log level, registry,
/// network settings), the absolute binary path the machine's probe resolved, and
/// the arguments. Built as argv words, never as a shell string.
fn remote_pnpm_argv(pnpm: &Path, layout: &InstallLayout, args: &[String]) -> Vec<String> {
    let mut argv = vec!["/usr/bin/env".to_string()];
    for (key, value) in &layout.extra_env {
        argv.push(format!("{key}={value}"));
    }
    for (key, value) in crate::toolchain::pnpm_network_env() {
        argv.push(format!("{key}={value}"));
    }
    // No TTY under the launcher: keep pnpm non-interactive so a modules purge
    // (store relink) never aborts with ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY.
    argv.push("CI=true".to_string());
    argv.push(pnpm.to_string_lossy().to_string());
    argv.extend(args.iter().cloned());
    argv
}

/// The exact command one npm install attempt spawns on a remote host: that
/// host's environment, the absolute pnpm its own probe resolved, the launcher's
/// install arguments against that host's store and registry, and the wrapper
/// that records the exit status next to the output (nothing can `wait` for a
/// detached process).
fn remote_npm_install_argv(pnpm: &Path, layout: &InstallLayout, version: &str) -> Vec<String> {
    crate::remote::measured_argv(
        &remote_pnpm_argv(
            pnpm,
            layout,
            &crate::toolchain::pnpm_install_args(
                &layout.dir,
                &layout.store_dir,
                layout.registry.as_deref(),
                &format!("@deepseek-ai/dsh@{version}"),
            ),
        ),
        &layout.dir.to_string_lossy(),
    )
}

/// The argv for one git command on a remote host. `git` is not one of the
/// tools the launcher probes, so it resolves through that machine's own login
/// shell `PATH` — the same one the user's terminal there has.
fn remote_git_argv(layout: &InstallLayout, args: &[String]) -> Vec<String> {
    let mut argv = vec!["/usr/bin/env".to_string()];
    for (key, value) in &layout.extra_env {
        argv.push(format!("{key}={value}"));
    }
    // Never prompt for credentials on a public repo.
    argv.push("GIT_TERMINAL_PROMPT=0".to_string());
    argv.push("CI=true".to_string());
    argv.push("git".to_string());
    argv.extend(args.iter().cloned());
    argv
}

/// The log one remote command of a task writes to: under that host's own data
/// directory, one file per command so a retry does not re-read the attempt
/// before it.
fn remote_log_path(data_dir: &Path, task_id: &str, step: &str) -> String {
    data_dir
        .join("logs")
        .join(format!("install-{task_id}-{step}.log"))
        .to_string_lossy()
        .to_string()
}

/// Runs one command on a remote host, detached, following everything it writes
/// into the task log until it is gone, and returns its exit status.
///
/// Detached because the install has to outlive the ssh session that started it
/// (ADR-0007), which is also why its output is a log file the launcher reads
/// rather than a pipe: [`crate::remote::tail`] waits *there* for output, so a
/// long quiet install is not one connection per second.
async fn run_remote_streamed(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    target: &crate::ssh::RemoteHost,
    log_path: &str,
    argv: Vec<String>,
    what: &str,
) -> Result<i32, String> {
    // Echo the exact command line, as the local streamed runner does: when a
    // child fails silently, the invocation itself is the only clue.
    push_task_log(app, state, task_id, &format!("$ {}", argv.join(" "))).await;

    let transport = state.host_transport.clone();
    let run = spawn_remote_run(state, target, &argv, log_path)
        .await
        .map_err(|e| format!("{what} 启动失败：{e}"))?;
    {
        let mut tasks = state.tasks.lock().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.remote = Some(run.clone());
        }
    }

    let outcome =
        follow_remote_log(app, state, task_id, &transport, &run, log_path, what).await;
    clear_remote_run(state, task_id).await;
    outcome?.ok_or_else(|| format!("{what} 被强制终止，没有留下退出码"))
}

/// Reads a remote command's log until it says how it ended: the wrapper's exit
/// status line, or the process being gone without one. `Ok(None)` means it ended
/// without saying (the tree was killed, which on a small server is usually the
/// out-of-memory killer).
async fn follow_remote_log(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    transport: &Arc<dyn crate::ssh::HostTransport>,
    run: &RemoteRun,
    log_path: &str,
    what: &str,
) -> Result<Option<i32>, String> {
    let mut offset = 0u64;
    let mut pending = String::new();
    let mut status = None;
    loop {
        let chunk = crate::remote::tail(
            &**transport,
            &run.host,
            log_path,
            offset,
            run.pid,
            REMOTE_TAIL_WAIT_SECS,
        )
        .await
        .map_err(|e| format!("{what} 读取远端日志失败：{e}"))?;
        offset = chunk.offset;
        pending.push_str(&chunk.text);
        status = status.or(drain_log_lines(app, state, task_id, &mut pending).await);

        if status.is_some() {
            return Ok(status);
        }
        if !chunk.alive {
            // The wrapper's status line is written by the shell that just
            // exited, so it can still be in flight when the process is already
            // gone. One read with no wait, and whatever is there is all there is.
            let last = crate::remote::tail(&**transport, &run.host, log_path, offset, run.pid, 0)
                .await
                .map_err(|e| format!("{what} 读取远端日志失败：{e}"))?;
            pending.push_str(&last.text);
            let status = status.or(drain_log_lines(app, state, task_id, &mut pending).await);
            return Ok(status);
        }
        if !task_is_running(state, task_id).await {
            // Cancelled: the click already stopped the tree on that machine,
            // and this is the worker noticing. Not a failure to report.
            return Err("任务已取消".to_string());
        }
    }
}

async fn clear_remote_run(state: &State<'_, AppState>, task_id: &str) {
    let mut tasks = state.tasks.lock().await;
    if let Some(task) = tasks.get_mut(task_id) {
        task.remote = None;
    }
}

/// Splits newly read log text into whole lines, logging each one: a chunk can
/// end mid-line, and half a line is not something anyone can read. Returns the
/// exit status if the wrapper's status line was among them.
async fn drain_log_lines(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
    pending: &mut String,
) -> Option<i32> {
    let mut status = None;
    while let Some(cut) = pending.find('\n') {
        let line = pending[..cut].trim_end_matches('\r').to_string();
        pending.drain(..=cut);
        if let Some(code) = crate::remote::exit_status_of(&line) {
            status = Some(code);
        } else if !line.trim().is_empty() {
            push_task_log(app, state, task_id, &line).await;
        }
    }
    status
}

/// Writes a version directory's workspace manifest on a remote host, from what
/// is already there: the same document [`crate::plugins::ensure_build_scripts_allowed`]
/// writes locally, produced by the same function.
async fn ensure_remote_build_scripts_allowed(
    state: &State<'_, AppState>,
    target: &crate::ssh::RemoteHost,
    dir: &Path,
) -> Result<(), String> {
    let manifest = dir.join("pnpm-workspace.yaml");
    let path = manifest.to_string_lossy().to_string();
    let raw = state
        .host_transport
        .read_file(target, &path)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default();
    let Some(document) = crate::plugins::build_scripts_manifest(&raw) else {
        return Ok(());
    };
    state
        .host_transport
        .write_file(target, &path, document.into_bytes())
        .await
        .map_err(|e| format!("写入那台机器上的 pnpm-workspace.yaml 失败：{e}"))
}

/// Starts one remote command as a session-detached process and returns where it
/// runs and the pid that leads its process group.
async fn spawn_remote_run(
    state: &AppState,
    target: &crate::ssh::RemoteHost,
    argv: &[String],
    log_path: &str,
) -> Result<RemoteRun, crate::ssh::HostError> {
    let pid = state
        .host_transport
        .spawn_detached(target, argv, log_path)
        .await?;
    Ok(RemoteRun {
        host: target.clone(),
        pid,
    })
}

/// Stops one remote install tree and waits until it is **really** gone.
///
/// Two things have to be true before the launcher may say 「已取消」: the whole
/// process group was signalled (a pnpm helper left behind holds the store's
/// lock, which is why [`crate::ssh::HostTransport::stop_group`] exists), and
/// nothing of it is left. Only then is the exit real.
async fn stop_remote_run(state: &AppState, run: &RemoteRun) -> Result<(), String> {
    let transport = state.host_transport.clone();
    transport
        .stop_group(&run.host, run.pid)
        .await
        .map_err(|e| format!("没能停掉那台机器上的安装进程（pid {}）：{e}", run.pid))?;

    let deadline = tokio::time::Instant::now() + REMOTE_STOP_TIMEOUT;
    while crate::remote::alive(&*transport, &run.host, run.pid)
        .await
        .map_err(|e| format!("没能确认那台机器上的安装进程是否已停止（pid {}）：{e}", run.pid))?
    {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "那台机器上的安装进程（pid {}）在 {} 秒内没有退出，它可能还在跑；请稍后重试，或到那台机器上处理",
                run.pid,
                REMOTE_STOP_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(REMOTE_STOP_TICK).await;
    }
    Ok(())
}

/// Removes one directory on a remote host, and refuses anything outside that
/// host's data directory.
///
/// The launcher removes only what it can prove it created **on that machine**
/// (ADR-0006): a remote path spelled like a local one must never make it delete
/// something on this Mac, and a cleanup that reaches outside the host's own data
/// directory on that machine is the same mistake in the other direction. Best
/// effort, like the local cleanup: a failure is logged, not raised.
async fn remove_remote_dir(
    state: &State<'_, AppState>,
    target: &crate::ssh::RemoteHost,
    path: &Path,
    data_dir: &Path,
) {
    if !inside(data_dir, path) {
        crate::log_warn!(
            "拒绝删除远端数据目录之外的路径 {}（{}）",
            path.display(),
            data_dir.display()
        );
        return;
    }
    if let Err(e) =
        crate::remote::remove_dir(&*state.host_transport, target, &path.to_string_lossy()).await
    {
        crate::log_warn!("清理远端目录 {} 失败: {e}", path.display());
    }
}

/// Whether `path` is inside `root` and not `root` itself. Decided by text,
/// because it has to hold for a path the launcher built without touching that
/// machine's filesystem — and it is the one rule that keeps a remote cleanup
/// inside the host that owns the directory.
pub(crate) fn inside(root: &Path, path: &Path) -> bool {
    let root = root.to_string_lossy();
    let path = path.to_string_lossy();
    if path.split('/').any(|part| part == "..") {
        return false;
    }
    let root = root.trim_end_matches('/');
    !root.is_empty() && path.starts_with(&format!("{root}/"))
}

/// What a remote install failure comes down to. Two of these look alike in a
/// progress bar and need opposite fixes, so the launcher tells them apart from
/// the log that machine produced (issue #9 风险): a small server that ran out of
/// memory, and a machine that could not reach the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteInstallFailure {
    OutOfMemory,
    Network,
    Other,
}

/// Words that mean the machine ran out of memory. `Killed` is what a shell
/// prints for a child the OOM killer took, and 137 is that child's exit status.
const OUT_OF_MEMORY_WORDS: [&str; 6] = [
    "out of memory",
    "cannot allocate memory",
    "enomem",
    "oom-kill",
    "allocation failed",
    "killed",
];

/// Words that mean the machine could not reach the registry. These are the
/// errors the tools themselves produce, not the word 「网络」.
const NETWORK_WORDS: [&str; 12] = [
    "enotfound",
    "econnrefused",
    "econnreset",
    "econnaborted",
    "etimedout",
    "eai_again",
    "esockettimedout",
    "getaddrinfo",
    "no such host",
    "temporary failure in name resolution",
    "network is unreachable",
    "socket hang up",
];

/// Reads a failure out of what the install said. Memory evidence comes first:
/// an install that hit the memory ceiling may well have network retries in its
/// log on the way there, and the memory is what it cannot get past.
pub(crate) fn classify_remote_install_failure(
    log: &str,
    exit_code: Option<i32>,
) -> RemoteInstallFailure {
    let said = log.to_lowercase();
    if OUT_OF_MEMORY_WORDS.iter().any(|word| said.contains(word)) {
        return RemoteInstallFailure::OutOfMemory;
    }
    if NETWORK_WORDS.iter().any(|word| said.contains(word)) {
        return RemoteInstallFailure::Network;
    }
    match exit_code {
        // Killed by a signal (SIGKILL / SIGABRT / SIGSEGV): on a small server
        // the usual reason an install dies without a word of its own is the
        // out-of-memory killer.
        Some(137) | Some(134) | Some(139) => RemoteInstallFailure::OutOfMemory,
        _ => RemoteInstallFailure::Other,
    }
}

/// The sentence a failed remote install leaves behind: which of the two classes
/// it was, what that verdict rests on, and where the whole log is — the log
/// itself is on that machine, and the launcher does not copy it here.
fn remote_install_failure(logs: &[String], exit_code: Option<i32>, log_path: &str) -> String {
    let log = logs.join("\n");
    let class = classify_remote_install_failure(&log, exit_code);
    let cited = match class {
        RemoteInstallFailure::OutOfMemory => failure_evidence(&log, &OUT_OF_MEMORY_WORDS),
        RemoteInstallFailure::Network => failure_evidence(&log, &NETWORK_WORDS),
        RemoteInstallFailure::Other => None,
    };
    let where_to_look = format!("完整日志在那台机器的 {log_path}");
    match class {
        RemoteInstallFailure::OutOfMemory => format!(
            "pnpm install 失败：那台机器的内存不够（{}）。给它加一块 swap、关掉别的吃内存的进程，\
             或换一台内存更大的机器；{where_to_look}",
            cited.unwrap_or_else(|| "安装进程被系统杀掉了".to_string())
        ),
        RemoteInstallFailure::Network => format!(
            "pnpm install 失败：那台机器连不上 registry（{}）。确认它能访问 npm：在那台机器上跑一次 \
             npm ping，或者在主机记录里填一个它连得上的 registry；{where_to_look}",
            cited.unwrap_or_else(|| "网络请求失败".to_string())
        ),
        RemoteInstallFailure::Other => {
            let tail = last_meaningful_lines(&log, 3);
            if tail.is_empty() {
                format!("pnpm install 退出码 {}，那台机器没有输出任何日志；{where_to_look}", exit_code.unwrap_or(-1))
            } else {
                format!("pnpm install 失败：{tail}｜{where_to_look}")
            }
        }
    }
}

/// The log line a verdict rests on, so the user can see the evidence instead of
/// taking the launcher's word for it.
fn failure_evidence(log: &str, words: &[&str]) -> Option<String> {
    log.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && {
            let said = line.to_lowercase();
            words.iter().any(|word| said.contains(word))
        })
        .map(shorten_line)
}

/// The last few lines with something in them, oldest first.
fn last_meaningful_lines(log: &str, count: usize) -> String {
    let mut lines: Vec<String> = log
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .take(count)
        .map(shorten_line)
        .collect();
    lines.reverse();
    lines.join(" | ")
}

fn shorten_line(line: &str) -> String {
    const MAX: usize = 160;
    if line.chars().count() <= MAX {
        return line.to_string();
    }
    line.chars().take(MAX).collect::<String>() + "…"
}

async fn task_logs(state: &State<'_, AppState>, task_id: &str) -> Vec<String> {
    let tasks = state.tasks.lock().await;
    tasks
        .get(task_id)
        .map(|task| task.logs.clone())
        .unwrap_or_default()
}

fn mentions_ignored_builds(logs: &[String]) -> bool {
    logs.iter().any(|line| {
        line.contains("ERR_PNPM_IGNORED_BUILDS") || line.contains("Ignored build scripts")
    })
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
        cmd.args(crate::toolchain::pnpm_install_args(
            dir,
            store_dir,
            crate::toolchain::registry_mirror().as_deref(),
            &format!("@deepseek-ai/dsh@{version}"),
        ));
        crate::toolchain::apply_pnpm_network_env(&mut cmd);
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

/// Records an installed version in the config (idempotent by version string
/// **per host**: the same version string on two machines is two installs).
fn register_version(
    state: &State<'_, AppState>,
    version: &str,
    dir: PathBuf,
    host_id: &str,
) -> Result<DshVersion, String> {
    let record = DshVersion {
        id: new_id("v"),
        version: version.to_string(),
        dir,
        host_id: host_id.to_string(),
    };
    let mut cfg = state.config.lock().unwrap();
    if let Some(existing) = cfg
        .versions
        .iter()
        .find(|v| v.version == *version && v.host_id == host_id)
    {
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
    register_version(state, version, dir, crate::config::LOCAL_HOST_ID)
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
        .map(|logs| mentions_ignored_builds(&logs))
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

    // -----------------------------------------------------------------------
    // Installing on a remote host
    // -----------------------------------------------------------------------

    use crate::remote::testing::{marked, FakeAnswer, FakeCall, FakeTransport};
    use crate::ssh::RemoteHost;
    use std::sync::Arc;

    const REMOTE_DATA_DIR: &str = "/home/demo/.local/share/dsh-launcher";

    fn remote_host() -> Host {
        Host {
            id: "host-lab".into(),
            display_name: "服务器".into(),
            ssh_target: "lab".into(),
            registry: Some("https://mirror.example/npm".into()),
            extra_env: [(
                "HTTPS_PROXY".to_string(),
                "http://10.0.0.1:8080".to_string(),
            )]
            .into_iter()
            .collect(),
            data_dir: Some(PathBuf::from(REMOTE_DATA_DIR)),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    fn remote_target() -> RemoteHost {
        RemoteHost {
            id: "host-lab".into(),
            target: "lab".into(),
            socket: PathBuf::from("/tmp/dsh-launcher-t5.sock"),
        }
    }

    fn state_with(transport: Arc<dyn crate::ssh::HostTransport>) -> AppState {
        AppState {
            config_path: PathBuf::from("/tmp/dsh-launcher-t5/config.json"),
            data_dir: PathBuf::from("/tmp/dsh-launcher-t5"),
            config: std::sync::Mutex::new(crate::config::Config::default()),
            running: tokio::sync::Mutex::new(Default::default()),
            tasks: tokio::sync::Mutex::new(Default::default()),
            host_transport: transport,
            host_statuses: Default::default(),
        }
    }

    /// The local install is what it was: the same three paths under the
    /// launcher's own data directory, the same registry source, and no extra
    /// environment (本机's network exit is the launcher's own proxy).
    #[test]
    fn the_local_install_keeps_the_paths_and_registry_it_always_had() {
        let data_dir = Path::new("/Users/me/Library/Application Support/in.dsh-plug.dsh-launcher");
        let layout = install_layout(&Host::local(), data_dir, "0.1.0-rc.6").unwrap();

        assert_eq!(layout.versions_dir, data_dir.join("versions"));
        assert_eq!(layout.dir, data_dir.join("versions").join("0.1.0-rc.6"));
        assert_eq!(layout.store_dir, data_dir.join(".pnpm-store"));
        assert_eq!(layout.data_dir, data_dir);
        assert!(layout.extra_env.is_empty(), "本机不带远端那套额外环境变量");
        assert_eq!(layout.registry, crate::toolchain::registry_mirror());
    }

    /// Everything an install touches on another machine is that machine's: the
    /// data directory its record names, and the registry its record names.
    #[test]
    fn a_remote_install_uses_that_hosts_paths_registry_and_environment() {
        let layout = install_layout(&remote_host(), Path::new("/launcher"), "0.1.0-rc.6").unwrap();

        assert_eq!(layout.data_dir, Path::new(REMOTE_DATA_DIR));
        assert_eq!(
            layout.dir,
            Path::new(&format!("{REMOTE_DATA_DIR}/versions/0.1.0-rc.6"))
        );
        assert_eq!(
            layout.store_dir,
            Path::new(&format!("{REMOTE_DATA_DIR}/.pnpm-store"))
        );
        assert_eq!(
            layout.versions_dir,
            Path::new(&format!("{REMOTE_DATA_DIR}/versions"))
        );
        assert_eq!(layout.registry.as_deref(), Some("https://mirror.example/npm"));
        assert_eq!(
            layout.extra_env,
            vec![(
                "HTTPS_PROXY".to_string(),
                "http://10.0.0.1:8080".to_string()
            )]
        );
    }

    /// What the machine is actually asked to run: the host's own environment and
    /// registry, the absolute pnpm its own probe resolved, that host's store and
    /// version directory — and the wrapper that records the exit status, because
    /// a detached process is nobody's child.
    #[tokio::test]
    async fn a_remote_install_spawns_that_hosts_own_command() {
        let layout = install_layout(&remote_host(), Path::new("/launcher"), "0.1.0-rc.6").unwrap();
        let pnpm = Path::new("/home/demo/.local/share/pnpm/pnpm");
        let argv = remote_npm_install_argv(pnpm, &layout, "0.1.0-rc.6");

        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert!(
            argv[2].starts_with(&format!("mkdir -p '{REMOTE_DATA_DIR}/versions/0.1.0-rc.6'")),
            "要在那台机器上建好版本目录：{}",
            argv[2]
        );
        assert!(argv[2].contains("<<<DSH-EXIT>>>"), "退出码要写进日志：{}", argv[2]);
        assert!(
            argv[2].contains("/home/demo/.local/share/pnpm/pnpm"),
            "工具用探到的绝对路径：{}",
            argv[2]
        );

        let fake = FakeTransport::new().answering(FakeAnswer::Pid(4242));
        let state = state_with(fake.clone());
        let log = remote_log_path(&layout.data_dir, "t-1", "npm-1");

        let run = spawn_remote_run(&state, &remote_target(), &argv, &log)
            .await
            .expect("要能起一个脱离会话的远端进程");
        assert_eq!(run.pid, 4242);

        let calls = fake.calls();
        let script = match calls.last() {
            Some(FakeCall::Spawn(script)) => script.clone(),
            other => panic!("排错了：{other:?}"),
        };
        assert!(script.contains("nohup setsid"), "{script}");
        assert!(
            script.contains("HTTPS_PROXY=http://10.0.0.1:8080"),
            "主机记录里的额外环境变量要跟着命令走：{script}"
        );
        assert!(
            script.contains("/home/demo/.local/share/pnpm/pnpm"),
            "要用那台机器上探到的绝对路径：{script}"
        );
        assert!(
            script.contains("https://mirror.example/npm"),
            "registry 来自那条主机记录：{script}"
        );
        assert!(
            script.contains(&format!("{REMOTE_DATA_DIR}/.pnpm-store")),
            "store 在那台机器的数据目录下：{script}"
        );
        assert!(script.contains("@deepseek-ai/dsh@0.1.0-rc.6"), "{script}");
        assert!(
            script.contains(&log),
            "输出写到那台机器上的日志，启动器读它：{script}"
        );
        assert!(
            !script.contains("/Users/"),
            "远端安装的脚本里不该有本机路径：{script}"
        );
    }

    /// Cancelling an install means the process tree up there is **gone** — not
    /// that the progress bar went away. The stop signals the group, and the
    /// launcher waits for the machine to agree before it says 「已取消」.
    #[tokio::test]
    async fn cancelling_waits_until_the_remote_install_tree_is_really_gone() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::Done)
            .answering(FakeAnswer::stdout(marked("{\"alive\":true}")))
            .answering(FakeAnswer::stdout(marked("{\"alive\":false}")));
        let state = state_with(fake.clone());
        let run = RemoteRun {
            host: remote_target(),
            pid: 4242,
        };

        stop_remote_run(&state, &run).await.expect("停掉之后要确认它没了");

        let calls = fake.calls();
        assert_eq!(
            calls[0],
            FakeCall::StopGroup(4242),
            "要按进程组停：只停组长会留下占着 store 锁的子进程"
        );
        assert!(calls.len() >= 2, "停完要确认它真的没了：{calls:?}");
        match &calls[1] {
            FakeCall::Run(script) => assert!(script.contains("dsh_pid=4242"), "{script}"),
            other => panic!("排错了：{other:?}"),
        }
    }

    /// A cancel that cannot reach the machine says the install may still be
    /// running, instead of collapsing the bar and pretending otherwise.
    #[tokio::test]
    async fn a_cancel_that_cannot_reach_the_host_says_so() {
        let fake = FakeTransport::new().answering(FakeAnswer::fail(
            crate::ssh::HostErrorKind::Unreachable,
            "ssh: connect to host lab port 22: Connection refused",
        ));
        let state = state_with(fake);
        let run = RemoteRun {
            host: remote_target(),
            pid: 4242,
        };

        let err = stop_remote_run(&state, &run).await.unwrap_err();
        assert!(err.contains("没能停掉"), "{err}");
        assert!(err.contains("4242"), "要点出是哪个进程：{err}");
    }

    /// A small server that ran out of memory and a machine that could not reach
    /// the network look the same in a progress bar and need opposite fixes.
    #[test]
    fn a_remote_install_that_ran_out_of_memory_is_told_apart_from_a_network_failure() {
        let log = "Progress: resolved 213, reused 0, downloaded 44\nKilled\n";
        let memory = remote_install_failure(
            &log.lines().map(str::to_string).collect::<Vec<_>>(),
            Some(137),
            &format!("{REMOTE_DATA_DIR}/logs/install-t-1-npm-3.log"),
        );
        assert!(memory.contains("内存不够"), "{memory}");
        assert!(memory.contains("Killed"), "理由要带上日志里的证据：{memory}");
        assert!(memory.contains(REMOTE_DATA_DIR), "要说清日志在哪：{memory}");

        let network = remote_install_failure(
            &[
                "npm http fetch GET 200 https://registry.npmjs.org/@deepseek-ai%2fdsh".to_string(),
                " ERR_PNPM_FETCH_ECONNREFUSED  request to https://mirror.example/npm failed: connect ECONNREFUSED".to_string(),
            ],
            Some(1),
            &format!("{REMOTE_DATA_DIR}/logs/install-t-1-npm-1.log"),
        );
        assert!(network.contains("连不上 registry"), "{network}");
        assert!(
            network.contains("ECONNREFUSED"),
            "理由要带上日志里的证据：{network}"
        );
        assert_ne!(memory, network, "两种失败必须给得出不同的理由");

        // Killed by a signal with nothing in the log: the exit status is the
        // only evidence there is, and it still says memory.
        assert_eq!(
            classify_remote_install_failure("", Some(137)),
            RemoteInstallFailure::OutOfMemory
        );
        assert_eq!(
            classify_remote_install_failure("pnpm: unknown option", Some(1)),
            RemoteInstallFailure::Other
        );
        let other = remote_install_failure(
            &["pnpm: unknown option".to_string()],
            Some(1),
            &format!("{REMOTE_DATA_DIR}/logs/install-t-1-npm-1.log"),
        );
        assert!(other.contains("unknown option"), "{other}");
        assert!(!other.contains("内存不够") && !other.contains("连不上"), "{other}");
    }

    /// The rule that keeps every remote cleanup inside the host that owns the
    /// directory: a path outside that host's data directory is never removed,
    /// however it is spelled.
    #[test]
    fn a_remote_cleanup_stays_inside_that_hosts_data_directory() {
        let root = Path::new(REMOTE_DATA_DIR);
        assert!(inside(root, &root.join("versions/0.1.0-rc.6")));
        assert!(inside(root, &root.join("homes/0.1.0-rc.6")));
        assert!(inside(root, &root.join(".pnpm-store")));
        assert!(!inside(root, root), "数据目录本身不是可以删的东西");
        assert!(!inside(root, Path::new("/home/demo/.dsh")));
        assert!(!inside(root, Path::new("/home/demo/.local/share/dsh-launcher-other")));
        assert!(
            !inside(root, &root.join("../secrets")),
            "带 .. 的路径一律拒绝"
        );
    }
}
