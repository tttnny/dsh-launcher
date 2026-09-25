use crate::config::{InstanceState, InstanceStatus};
use crate::AppState;
use regex::Regex;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, Notify};

pub const STATUS_EVENT: &str = "instance://status";

/// A live instance process.
pub struct RunningInstance {
    /// Stop signal. The waiter task owns the child and `select!`s on this, so
    /// `stop_instance_process` never touches the child handle directly.
    pub kill: Arc<Notify>,
    pub profile: String,
    pub url: Option<String>,
    /// Set when the running entry was adopted from a process the launcher did
    /// not spawn (e.g. DSH restarted itself): `kill` is unused and the watcher
    /// polls `pid` for liveness instead of reaping a child.
    pub adopted: Option<Adopted>,
}

#[derive(Clone, Debug)]
pub struct Adopted {
    pub pid: i32,
    pub port: u16,
    pub host: String,
}

/// One listener on the instance port, as reported by lsof.
struct PortOwner {
    pid: i32,
    command: String,
}

/// Find the process listening on `port`. `None` = port free (or probe
/// failed — degrade to the old behavior: spawn and let DSH's own
/// EADDRINUSE speak).
async fn probe_port_owner(port: u16) -> Option<PortOwner> {
    let mut cmd = Command::new("lsof");
    hide_console(&mut cmd);
    // `-i` must be a single `-iTCP:<port>` argument: a separate "3080" is
    // parsed as a path operand. No host inside it (lsof 4.91 rejects
    // `host:port` there); any local listener on the port would collide with
    // the pinned bind anyway.
    let out = cmd
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp", "-a"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pid: Option<i32> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            pid = rest.parse().ok();
            break;
        }
    }
    let pid = pid?;
    let command = process_name(pid).await.unwrap_or_default();
    Some(PortOwner { pid, command })
}

/// The short command name (first argv element) of `pid`, best-effort.
async fn process_name(pid: i32) -> Option<String> {
    let mut cmd = Command::new("ps");
    hide_console(&mut cmd);
    let out = cmd
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether a port owner looks like a DSH web server: the launcher spawns
/// `node <…>/dsh/…/bin.js`, so the listener process is node/DSH's packaged
/// binary. We match loosely so a packaged executable is recognized too.
fn looks_like_dsh(command: &str) -> bool {
    let c = command.to_ascii_lowercase();
    c.contains("node") || c.contains("dsh")
}

/// Verify the adopted pid still exists (cheap `kill -0` poll).
fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill(2) with signal 0 performs the permission+existence check
    // without delivering anything.
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) }
}

/// Register an externally-owned DSH process (DSH restarted itself in-place)
/// as the instance's running entry: bare-URL (the token is in-memory in the
/// owning process and unrecoverable, but the 30-day browser cookie keeps the
/// existing browser session working), then start the liveness watcher.
async fn adopt_external_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
    profile: &str,
    port: u16,
    host: &str,
    pid: i32,
) {
    let url = format!("http://{host}:{port}");
    {
        let mut running = state.running.lock().await;
        if running.contains_key(instance_id) {
            return; // A real child (re)registered first; it wins.
        }
        running.insert(
            instance_id.to_string(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: profile.to_string(),
                url: Some(url.clone()),
                adopted: Some(Adopted { pid, port, host: host.to_string() }),
            },
        );
    }
    crate::log_info!(
        "实例 {instance_id} 端口 {host}:{port} 被 DSH 自身重启的外部进程占用（pid {pid}），已收养为运行中"
    );
    emit_status(
        app,
        &InstanceStatus {
            id: instance_id.to_string(),
            state: InstanceState::Running,
            url: Some(url),
            profile: Some(profile.to_string()),
            exit_code: None,
        },
    );
    crate::tray::rebuild_tray_menu(app).await;

    // Watcher: poll the external pid; when it dies, clean up like the waiter
    // does for real children.
    let watcher_app = app.clone();
    let watcher_id = instance_id.to_string();
    let watcher_profile = profile.to_string();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let state = watcher_app.state::<AppState>();
            // Stop coordinating when the entry was replaced by a real child
            // (a start raced our adoption) or the user stopped the instance.
            let still_ours = {
                let running = state.running.lock().await;
                running
                    .get(&watcher_id)
                    .map(|r| r.adopted.as_ref().map(|a| a.pid) == Some(pid))
                    .unwrap_or(false)
            };
            if !still_ours {
                return;
            }
            if pid_alive(pid) {
                continue;
            }
            crate::log_info!("实例 {watcher_id} 收养的外部进程（pid {pid}）已退出");
            unregister_running(
                &watcher_app,
                &state,
                &watcher_id,
                InstanceStatus {
                    id: watcher_id.clone(),
                    state: InstanceState::Exited,
                    url: None,
                    profile: Some(watcher_profile),
                    exit_code: None,
                },
            )
            .await;
            return;
        }
    });
}

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"dsh web: (https?://[^\s]+)").unwrap())
}

pub fn npm() -> &'static str {
    "npm"
}

pub fn node() -> &'static str {
    "node"
}

/// Helper for command building; on macOS Unix this is a no-op passthrough.
pub fn hide_console(cmd: &mut Command) -> &mut Command {
    cmd
}

/// Version layout + every DSH invocation shape lives in `crate::launch`.
pub use crate::launch::instance_env as build_env;
use crate::launch::{version_bin, version_bin_ready};

/// Whether a profile is a DSH web application: its package.json
/// `dsh.profile.bundles` includes `@deepseek-ai/dsh-web-app`. Such profiles
/// understand `--host/--port` and get a webview; they must bind a random
/// free port so several instances don't collide.
fn is_web_profile(home_path: &std::path::Path, profile: &str) -> bool {
    let pkg = crate::profile::profile_dir(home_path, profile)
        .ok()
        .map(|d| d.join("package.json"))
        .unwrap_or_else(|| home_path.join("profiles").join(profile).join("package.json"));
    let Ok(raw) = std::fs::read_to_string(&pkg) else {
        return false;
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    doc.pointer("/dsh/profile/bundles")
        .and_then(|b| b.as_array())
        .map(|arr| {
            arr.iter()
                .any(|b| b.as_str() == Some("@deepseek-ai/dsh-web-app"))
        })
        .unwrap_or(false)
}

/// Spawns a DSH CLI process for the instance/profile and starts the watchers
/// that parse the web URL, write logs and emit status events.
pub async fn start_instance_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
    profile: &str,
) -> Result<(), String> {
    let cfg = state.config.lock().unwrap().clone();
    let inst = cfg
        .instances
        .iter()
        .find(|i| i.id == instance_id)
        .cloned()
        .ok_or_else(|| "实例不存在".to_string())?;
    let version = cfg
        .versions
        .iter()
        .find(|v| v.id == inst.version_id)
        .ok_or_else(|| "实例引用的 DSH 版本未安装".to_string())?;

    let bin = version_bin(&version.dir);
    if !version_bin_ready(&version.dir) {
        return Err(format!(
            "版本 {} 安装不完整（缺少 {}），请重新安装",
            version.version,
            bin.display()
        ));
    }

    // Guard: already running/starting.
    {
        let running = state.running.lock().await;
        if running.contains_key(instance_id) {
            return Err("实例已在运行".to_string());
        }
    }

    // Web-app profiles (their bundle list includes @deepseek-ai/dsh-web-app)
    // get a random free port; other profiles are managed purely as processes
    // (no URL/webview). We detect the web bundle rather than relying on the
    // profile being literally named "web" so user-named web profiles work.
    let home_path = cfg
        .homes
        .iter()
        .find(|h| h.id == inst.home_id)
        .map(|h| h.path.clone());
    let is_web = home_path
        .as_deref()
        .map(|hp| is_web_profile(hp, profile))
        .unwrap_or(profile == "web");

    // Preflight for pinned-port instances: DSH can restart itself in-place
    // (its new process is not our child and holds the pinned port), leaving
    // the card "stopped" while the port stays occupied — a naive start would
    // then die with EADDRINUSE. When the port is held by a DSH-looking
    // process, adopt it as the running instance; when held by anything else,
    // fail with a actionable error instead of a crash loop.
    let mut pinned_port: Option<(String, u16)> = None;
    if is_web {
        if let Some(port) = inst.port {
            pinned_port = Some(("127.0.0.1".to_string(), port));
        }
    }
    if let Some((host, port)) = pinned_port.clone() {
        if let Some(owner) = probe_port_owner(port).await {
            if looks_like_dsh(&owner.command) {
                adopt_external_process(app, state, instance_id, profile, port, &host, owner.pid)
                    .await;
                return Ok(());
            }
            return Err(format!(
                "端口 {port} 已被其他进程占用（pid {}，{}），请先释放该端口或修改实例端口",
                owner.pid,
                if owner.command.is_empty() { "未知进程" } else { &owner.command }
            ));
        }
    }

    // Web-app profiles: the profile patch's managed lan-bind block outranks
    // `--port` and the plugin only re-asserts it at boot, so sync it here
    // first — otherwise a changed port would need a stop/start cycle to take
    // effect. (Pinned ports were handled by the preflight above.)
    if is_web {
        if let Some(hp) = home_path.as_deref() {
            crate::profile::assert_profile_lan_bind_port(hp, profile, inst.port.unwrap_or(0));
        }
    }

    // The interpreter is the resolved binding rather than a bare name, and a
    // toolchain installed while the app was running is picked up here (the
    // binding is re-probed when its recorded path no longer exists).
    let node = crate::runtime::node_for_spawn_checked(state).await?;

    // The launch spec module owns the argv: node_runtime_flags order,
    // --profile, per-instance --port, --no-open feature detection, and the
    // deliberate absence of --host (the profile layer owns the bind host).
    let mut cmd = crate::launch::instance_command(
        &node,
        &version.dir,
        profile,
        is_web.then(|| inst.port.unwrap_or(0)),
        cfg.settings.preserve_symlinks,
    )?;

    let env = build_env(&cfg, instance_id)?;
    for (k, v) in env {
        cmd.env(k, v);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("启动进程失败: {e}"))?;
    crate::log_info!("实例 {instance_id} 进程已拉起（profile: {profile}）");

    // Take the pipes before wrapping the child (the waiter takes ownership of
    // the child itself).
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let log_dir = state.data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    let log_path = log_dir.join(format!("{instance_id}.log"));
    let log_file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap_or_else(|_| {
                // Fallback: discard output if logging fails.
                OpenOptions::new()
                    .write(true)
                    .open(std::path::Path::new("NUL"))
                    .unwrap_or_else(|_| panic!("cannot open log file"))
            }),
    ));

    let kill_switch = Arc::new(Notify::new());

    emit_status(
        app,
        &InstanceStatus {
            id: instance_id.to_string(),
            state: InstanceState::Starting,
            url: None,
            profile: Some(profile.to_string()),
            exit_code: None,
        },
    );

    // Register the running entry (stop_instance reaches it through the map);
    // the tray menu is a projection of the registry.
    register_running(
        app,
        state,
        instance_id,
        RunningInstance {
            kill: kill_switch.clone(),
            profile: profile.to_string(),
            url: None,
            adopted: None,
        },
    )
    .await;

    // Waiter: owns the child, awaits exit or a kill request, then cleans up
    // and notifies. It is the single place that removes the map entry and
    // emits the terminal status, so state transitions never diverge.
    {
        let waiter_app = app.clone();
        let waiter_id = instance_id.to_string();
        let waiter_profile = profile.to_string();
        let waiter_kill = kill_switch.clone();
        let waiter_port = pinned_port;
        let mut child = child;
        tauri::async_runtime::spawn(async move {
            let state = waiter_app.state::<AppState>();
            let (code, stopped) = tokio::select! {
                status = child.wait() => (status.ok().and_then(|s| s.code()), false),
                _ = waiter_kill.notified() => {
                    let _ = child.kill().await;
                    let code = child.wait().await.ok().and_then(|s| s.code());
                    (code, true)
                }
            };
            // Quiet removal: the terminal status depends on the adoption
            // re-probe below, so projections are deferred until it resolves.
            remove_entry_quiet(&state, &waiter_id).await;
            if stopped {
                crate::log_info!("实例 {waiter_id} 已停止（exit code: {code:?}）");
            } else {
                crate::log_warn!("实例 {waiter_id} 意外退出（exit code: {code:?}）");
            }
            // An unexpected exit may be DSH restarting itself: its successor
            // is not our child but already re-bound the pinned port. Re-probe
            // briefly (the successor needs a moment to bind) and adopt it so
            // the card converges to "running" instead of dead-ending on
            // EADDRINUSE at the next manual start.
            if !stopped {
                if let Some((host, port)) = waiter_port {
                    for attempt in 0..10 {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if let Some(owner) = probe_port_owner(port).await {
                            if looks_like_dsh(&owner.command) {
                                adopt_external_process(
                                    &waiter_app,
                                    &state,
                                    &waiter_id,
                                    &waiter_profile,
                                    port,
                                    &host,
                                    owner.pid,
                                )
                                .await;
                                return;
                            }
                        }
                        if attempt == 9 {
                            crate::log_info!(
                                "实例 {waiter_id} 退出后端口 {host}:{port} 未见 DSH 进程，按普通退出处理"
                            );
                        }
                    }
                }
            }
            unregister_running(
                &waiter_app,
                &state,
                &waiter_id,
                InstanceStatus {
                    id: waiter_id.clone(),
                    state: if stopped {
                        InstanceState::Stopped
                    } else {
                        InstanceState::Exited
                    },
                    url: None,
                    profile: Some(waiter_profile),
                    exit_code: code,
                },
            )
            .await;
        });
    }

    // stdout watcher: parse `dsh web: <url>` and emit "running".
    if let Some(out) = stdout {
        let reader_app = app.clone();
        let reader_id = instance_id.to_string();
        let reader_profile = profile.to_string();
        let reader_log = log_file.clone();
        tauri::async_runtime::spawn(async move {
            let state = reader_app.state::<AppState>();
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log_line(&reader_log, &line).await;
                if let Some(cap) = url_re().captures(&line) {
                    let url = cap[1].to_string();
                    crate::log_info!("实例 {reader_id} 已就绪：{url}");
                    {
                        let mut running = state.running.lock().await;
                        if let Some(entry) = running.get_mut(&reader_id) {
                            entry.url = Some(url.clone());
                        }
                    }
                    emit_status(
                        &reader_app,
                        &InstanceStatus {
                            id: reader_id.clone(),
                            state: InstanceState::Running,
                            url: Some(url),
                            profile: Some(reader_profile.clone()),
                            exit_code: None,
                        },
                    );
                }
            }
        });
    }

    // stderr watcher: forward to the log (diagnostics).
    if let Some(err) = stderr {
        let reader_log = log_file.clone();
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log_line(&reader_log, &line).await;
            }
        });
    }

    Ok(())
}

/// Stops a running instance: signals the waiter task to kill the child and
/// waits for it to reap the process and remove the registry entry. The stop
/// is idempotent: when the entry is already gone (crashed, stopped from the
/// tray, duplicate click) a terminal status is re-emitted so a stale
/// frontend converges instead of erroring with "实例未在运行".
pub async fn stop_instance_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
) -> Result<(), String> {
    let entry = state
        .running
        .lock()
        .await
        .get(instance_id)
        .map(|r| (r.kill.clone(), r.adopted.clone()));
    let Some((kill, adopted)) = entry else {
        crate::log_debug!("停止实例 {instance_id}：注册表无记录，补发 stopped 状态");
        emit_status(
            app,
            &InstanceStatus {
                id: instance_id.to_string(),
                state: InstanceState::Stopped,
                url: None,
                profile: None,
                exit_code: None,
            },
        );
        return Ok(());
    };
    if let Some(a) = adopted {
        // Adopted external process: no child handle, terminate by pid. The
        // watcher notices the exit and emits the terminal status.
        crate::log_info!("停止实例 {instance_id}：结束收养的外部进程（pid {}）", a.pid);
        let kill_result = unsafe { libc::kill(a.pid, libc::SIGTERM) };
        if kill_result != 0 && !pid_alive(a.pid) {
            return Ok(());
        }
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if !state.running.lock().await.contains_key(instance_id) {
                return Ok(());
            }
        }
        return Err("停止实例超时（外部进程未响应终止信号）".to_string());
    }
    crate::log_info!("收到停止实例 {instance_id} 的请求");
    kill.notify_one();
    // Wait for the waiter task to finish cleanup so callers (e.g. the restart
    // flow) observe a clean registry on return.
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if !state.running.lock().await.contains_key(instance_id) {
            return Ok(());
        }
    }
    crate::log_error!("停止实例 {instance_id} 超时：进程未响应终止信号");
    Err("停止实例超时（进程未响应终止信号）".to_string())
}

/// Kills every running instance (called on launcher exit). Best-effort: the
/// waiter tasks reap the children asynchronously as the runtime shuts down.
pub fn kill_all(state: &AppState) {
    let running = state.running.blocking_lock();
    for entry in running.values() {
        if let Some(a) = &entry.adopted {
            // Best-effort SIGTERM; the external process outlives the
            // launcher's exit anyway (it was never our child).
            unsafe {
                libc::kill(a.pid, libc::SIGTERM);
            }
            continue;
        }
        entry.kill.notify_one();
    }
}

/// The restart transition, owned by the lifecycle module instead of being
/// re-assembled by every caller: a full stop (which observes a clean
/// registry on return — see [`stop_instance_process`]) followed by a start
/// on the same profile.
pub async fn restart_instance_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
) -> Result<(), String> {
    let profile = state
        .running
        .lock()
        .await
        .get(instance_id)
        .map(|r| r.profile.clone());
    let Some(profile) = profile else {
        return Err("实例未在运行".to_string());
    };
    stop_instance_process(app, state, instance_id).await?;
    start_instance_process(app, state, instance_id, &profile).await
}

pub async fn list_statuses(state: &State<'_, AppState>) -> Vec<InstanceStatus> {
    let running = state.running.lock().await;
    running
        .iter()
        .map(|(id, entry)| InstanceStatus {
            id: id.clone(),
            state: InstanceState::Running,
            url: entry.url.clone(),
            profile: Some(entry.profile.clone()),
            exit_code: None,
        })
        .collect()
}

async fn log_line(log: &Arc<Mutex<std::fs::File>>, line: &str) {
    let mut f = log.lock().await;
    let _ = writeln!(f, "{line}");
    let _ = f.flush();
}

fn emit_status(app: &AppHandle, status: &InstanceStatus) {
    let _ = app.emit(STATUS_EVENT, status);
}

// ---------------------------------------------------------------------------
// Registry transitions — the lifecycle module's one rule: mutate the running
// table only through these helpers, and every mutation carries its
// projections (status event + tray menu). A new transition site cannot
// forget the choreography because there is no choreography.
// ---------------------------------------------------------------------------

/// Inserts (or replaces) a running entry and refreshes the tray menu. The
/// caller emits the instance's own status event (Starting / Running) around
/// it — those carry per-transition detail (url, exit code) this helper
/// doesn't know.
async fn register_running(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
    entry: RunningInstance,
) {
    state.running.lock().await.insert(id.to_string(), entry);
    crate::tray::rebuild_tray_menu(app).await;
}

/// Removes a running entry and projects the terminal status (status event +
/// tray menu). Not for the exit waiter, which must remove first and re-probe
/// adoption before deciding the terminal status — see [`remove_entry_quiet`].
async fn unregister_running(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
    status: InstanceStatus,
) {
    state.running.lock().await.remove(id);
    emit_status(app, &status);
    crate::tray::rebuild_tray_menu(app).await;
}

/// Removes a running entry WITHOUT projections: for the exit waiter, whose
/// terminal status depends on the adoption re-probe that follows.
async fn remove_entry_quiet(state: &State<'_, AppState>, id: &str) {
    state.running.lock().await.remove(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_dsh_matches_node_and_dsh_processes() {
        assert!(looks_like_dsh("node"));
        assert!(looks_like_dsh("Node.js"));
        assert!(looks_like_dsh("dsh"));
        assert!(looks_like_dsh("/path/to/dsh-bin"));
        assert!(!looks_like_dsh("Python"));
        assert!(!looks_like_dsh("Google Chrome"));
        assert!(!looks_like_dsh(""));
    }

    #[tokio::test]
    async fn probe_port_owner_finds_and_releases_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let owner = probe_port_owner(port).await;
        assert!(owner.is_some(), "probe must find the listener we hold");
        let owner = owner.unwrap();
        assert_eq!(owner.pid, std::process::id() as i32);
        drop(listener);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            probe_port_owner(port).await.is_none(),
            "probe must report a freed port"
        );
    }

    #[test]
    fn is_web_profile_detects_web_app_bundle() {
        let dir = std::env::temp_dir().join(format!("dsh-proc-test-{}", uuid::Uuid::new_v4()));
        let profile_dir = dir.join("profiles").join("test");
        std::fs::create_dir_all(&profile_dir).unwrap();
        // No package.json -> not a web profile.
        assert!(!is_web_profile(&dir, "test"));
        // Web app bundle -> web profile.
        std::fs::write(
            profile_dir.join("package.json"),
            r#"{"name":"dsh-profile-test","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app"]}}}"#,
        )
        .unwrap();
        assert!(is_web_profile(&dir, "test"));
        // Non-web profile (no dsh-web-app bundle) -> false.
        let other = dir.join("profiles").join("bot");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("package.json"),
            r#"{"name":"dsh-profile-bot","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base"]}}}"#,
        )
        .unwrap();
        assert!(!is_web_profile(&dir, "bot"));
        std::fs::remove_dir_all(&dir).ok();
    }

    
    
    }
