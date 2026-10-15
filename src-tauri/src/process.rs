use crate::config::{DshInstance, DshVersion, Host, InstanceState, InstanceStatus};
use crate::AppState;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
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
    /// Set when the process runs on another machine. Such an instance is
    /// nobody's child here: it is stopped by pid **there**, and it is meant to
    /// outlive this launcher (ADR-0007), which is what [`kill_all`] reads.
    pub remote: Option<RemoteRun>,
}

/// A DSH process the launcher started **on another machine**.
///
/// It replaces the child handle a local instance has: that machine's shell
/// started it in its own session (`setsid`), so nothing here can wait for it or
/// signal it directly. What the launcher knows instead is where it runs and what
/// it is allowed to say about it — the pid that leads its process group, the
/// process start time that tells a reused pid from this one, and that machine's
/// own data directory, log and running record.
#[derive(Clone, Debug)]
pub struct RemoteRun {
    pub host_id: String,
    /// The machine, as the transport dials it.
    pub host: crate::ssh::RemoteHost,
    /// The pid `setsid` started — also the id of the process group a stop
    /// signals as a whole.
    pub pid: i32,
    /// When that pid started, as its own machine's `/proc/<pid>/stat` reports it.
    /// `None` = the machine could not be asked, so a later stop cannot tell a
    /// reused pid apart and has to say so rather than signal blindly.
    pub started_at: Option<u64>,
    /// That host's data directory **on that machine**.
    pub data_dir: PathBuf,
    pub log_path: String,
}

/// One instance's entry in a remote host's own running record
/// (`<该主机的数据目录>/running.json`).
///
/// It is written **on that machine** because that is where the facts are: a
/// later launcher session (or a stop) finds here what it did not witness — the
/// URL with the launch token in it, the profile, and the pid together with its
/// process start time (ADR-0007 §3). Without the URL a running instance is only
/// half-known: the card can say 运行, but the plugins page has no token to talk
/// to it with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRunRecord {
    /// The URL the instance printed when it became ready, token included.
    /// `None` until it has printed one.
    #[serde(default)]
    pub url: Option<String>,
    /// The `token=` value out of that URL, on its own so a reader does not have
    /// to parse it back out.
    #[serde(default)]
    pub token: Option<String>,
    pub profile: String,
    pub pid: i32,
    /// `/proc/<pid>/stat` field 22, in clock ticks since boot.
    #[serde(default)]
    pub started_at: Option<u64>,
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

/// Register an externally-owned DSH process (DSH restarted itself in-place, or
/// the launcher was replaced while the instance kept running) as the
/// instance's running entry, then start the liveness watcher.
///
/// The token is in-memory in the owning process, but that process is usually
/// still the one that printed the URL we remembered — so the remembered URL is
/// tried first and adopted with it when the instance still accepts it (plugin
/// management keeps working). Only when there is no working reminder does the
/// entry degrade to the bare URL the page calls "由系统接管".
async fn adopt_external_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
    profile: &str,
    port: u16,
    host: &str,
    pid: i32,
) {
    let data_dir = state.data_dir.clone();
    let bare = format!("http://{host}:{port}");
    let remembered = match recall_url(&data_dir, instance_id) {
        Some(url) if crate::native::NativeClient::connect(&url).await.is_ok() => Some(url),
        Some(_) => {
            // The process that printed it is gone (DSH restarted itself): the
            // new one minted its own token, so the reminder is dead weight.
            forget_url(&data_dir, instance_id);
            None
        }
        None => None,
    };
    let url = remembered.clone().unwrap_or(bare);
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
                remote: None,
            },
        );
    }
    if remembered.is_some() {
        crate::log_info!(
            "实例 {instance_id} 端口 {host}:{port} 被仍在运行的外部进程占用（pid {pid}），已收养为运行中并沿用上次的启动 token"
        );
    } else {
        crate::log_info!(
            "实例 {instance_id} 端口 {host}:{port} 被 DSH 自身重启的外部进程占用（pid {pid}），已收养为运行中（无可用 token）"
        );
    }
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

/// The port of an instance URL (`http://127.0.0.1:3088/?token=…` → 3088), for
/// finding an instance whose port was not pinned.
fn port_of_url(url: &str) -> Option<u16> {
    reqwest::Url::parse(url.trim()).ok()?.port()
}

/// Starts one background pass that adopts the instances this launcher session
/// did *not* spawn.
///
/// A launcher that is replaced (app update) or force-quit leaves its DSH
/// processes running: they are not reaped, and nothing notices until the user
/// starts one again — which is exactly when the card says "已停止" while the
/// port is taken, and the plugins page degrades to a file listing. The pinned
/// port, or the port in the URL we remembered, is where to look.
pub async fn adopt_surviving_instances(app: &AppHandle) {
    let state = app.state::<AppState>();
    let data_dir = state.data_dir.clone();
    struct Candidate {
        id: String,
        profile: String,
        host: String,
        port: u16,
        remembered: Option<String>,
        /// A user-configured port is evidence by itself; a random one is only
        /// believed if the URL we remembered still authenticates.
        pinned: bool,
    }
    let candidates: Vec<Candidate> = {
        let cfg = state.config.lock().unwrap();
        cfg.instances
            .iter()
            // 本机 only: another machine's port is on that machine, and the
            // local probe below would be asking this Mac about it — a local
            // process wearing the same port number would be adopted as a remote
            // instance. Adopting there is that machine's own pass (ADR-0007).
            .filter(|inst| inst.host_id.trim().is_empty() || inst.host_id == crate::config::LOCAL_HOST_ID)
            .filter_map(|inst| {
                let remembered = recall_url(&data_dir, &inst.id);
                let port = inst.port.or_else(|| remembered.as_deref().and_then(port_of_url))?;
                // The remembered profile is what it was actually started with;
                // the stored defaults are the best guess after that.
                let profile = recall_profile(&data_dir, &inst.id)
                    .or_else(|| inst.last_profile.clone())
                    .or_else(|| inst.default_profile.clone())
                    .unwrap_or_else(|| "web".to_string());
                Some(Candidate {
                    id: inst.id.clone(),
                    profile,
                    host: "127.0.0.1".to_string(),
                    port,
                    pinned: inst.port.is_some(),
                    remembered,
                })
            })
            .collect()
    };
    for c in candidates {
        let Some(owner) = probe_port_owner(c.port).await else {
            continue;
        };
        if !looks_like_dsh(&owner.command) {
            continue;
        }
        if !c.pinned {
            let authenticates = match c.remembered.as_deref() {
                Some(url) => crate::native::NativeClient::connect(url).await.is_ok(),
                None => false,
            };
            if !authenticates {
                continue;
            }
        }
        adopt_external_process(app, &state, &c.id, &c.profile, c.port, &c.host, owner.pid).await;
    }
}

// ---------------------------------------------------------------------------
// Remembered launch URLs
//
// DSH's launch token lives in the process and is unrecoverable from outside —
// but the *URL that carries it* is ours: the instance prints it on its own
// stdout. Persisting it is what lets a later launcher session adopt a DSH
// process that outlived us (an app update, a quit-and-reopen) **and still
// manage its plugins**, instead of degrading to the dead-end "由系统接管"
// state for the rest of that process's life.
//
// The file is a hint, never an authority: a remembered token is only trusted
// after the instance accepts it, and it is dropped the moment it does not
// (DSH restarted itself in place, which mints a new token).
// ---------------------------------------------------------------------------

/// `<data_dir>/running.json` — remembered launch URLs, keyed by instance id.
fn remembered_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("running.json")
}

fn read_remembered(data_dir: &std::path::Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(remembered_path(data_dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_remembered(
    data_dir: &std::path::Path,
    map: &serde_json::Map<String, serde_json::Value>,
) {
    let path = remembered_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let Ok(raw) = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone())) else {
        return;
    };
    if std::fs::write(&tmp, raw).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Records the URL an instance printed when it became ready.
pub(crate) fn remember_url(data_dir: &std::path::Path, instance_id: &str, url: &str, profile: &str) {
    let mut map = read_remembered(data_dir);
    map.insert(
        instance_id.to_string(),
        serde_json::json!({ "url": url, "profile": profile }),
    );
    write_remembered(data_dir, &map);
}

/// Forgets an instance's remembered URL (stopped, exited, or a stale token).
pub(crate) fn forget_url(data_dir: &std::path::Path, instance_id: &str) {
    let mut map = read_remembered(data_dir);
    if map.remove(instance_id).is_some() {
        write_remembered(data_dir, &map);
    }
}

/// The URL we last saw this instance print, if we ever did.
pub(crate) fn recall_url(data_dir: &std::path::Path, instance_id: &str) -> Option<String> {
    read_remembered(data_dir)
        .get(instance_id)
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// The profile that instance was running when it printed that URL.
pub(crate) fn recall_profile(data_dir: &std::path::Path, instance_id: &str) -> Option<String> {
    read_remembered(data_dir)
        .get(instance_id)
        .and_then(|v| v.get("profile"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// The URL the launcher can actually authenticate with, if any. An entry
/// without a token (an adopted process we never saw print one) is *running* but
/// not manageable; the reverse — adopted with a token — is both.
pub(crate) fn manageable_url(entry: &RunningEntry) -> Option<&str> {
    entry.url.as_deref().filter(|u| u.contains("token="))
}

// ---------------------------------------------------------------------------
// Running an instance on another machine
//
// A remote instance is not this launcher's child. That machine's own shell
// starts it in its own session, with `/dev/null` for stdin and its output
// appended to a log file there, so it is still running after the ssh connection
// closes and after the launcher itself quits (ADR-0007). What the launcher keeps
// in place of a child handle is [`RemoteRun`] — which machine, which pid — and
// what keeps it manageable across launcher restarts is the URL, launch token
// included, written into **that machine's** own running record the moment its
// log prints it.
//
// Nothing here reads a profile, a version tree or a log from this Mac: those are
// paths on the other machine, and a local `fs` call aimed at one of them would
// answer about whatever this Mac happens to have at the same spelling (ADR-0006).
// ---------------------------------------------------------------------------

/// How long one read of a remote instance's log may wait **on that machine** for
/// output, for the process to go, or for the budget. The usual case — nothing
/// new is written for minutes — costs one call per window instead of one per
/// second; output that appears is seen about a second later.
const REMOTE_TAIL_WAIT_SECS: u64 = 10;

/// How long to wait before reading a remote log again after the machine could
/// not answer. A machine that went away comes back; the follower is what notices
/// either way, so a failed read is not the end of the watch.
const REMOTE_FOLLOW_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// `<该主机的数据目录>/running.json` — the file the records above live in, as
/// that machine spells the path.
fn remote_record_path(data_dir: &Path) -> String {
    data_dir.join("running.json").to_string_lossy().to_string()
}

/// Reading that file, changing one entry and writing it back is two round trips,
/// so two instances starting at once could drop each other's entry. One lock for
/// every host is enough: these writes are rare and tiny.
static REMOTE_RECORD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One host's running record as it is on that machine. A file that is not there
/// yet is an empty record: the launcher is the one that creates it.
async fn read_remote_records(
    state: &AppState,
    run: &RemoteRun,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let path = remote_record_path(&run.data_dir);
    let bytes = crate::remote::read_file_optional(&*state.host_transport, &run.host, &path)
        .await
        .map_err(|e| format!("读取那台机器上的运行记录失败（{path}）：{e}"))?;
    let Some(bytes) = bytes else {
        return Ok(serde_json::Map::new());
    };
    let text = String::from_utf8_lossy(&bytes).to_string();
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .ok_or_else(|| format!("那台机器上的运行记录不是对象（{path}）"))
}

async fn write_remote_records(
    state: &AppState,
    run: &RemoteRun,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let path = remote_record_path(&run.data_dir);
    let raw = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
        .map_err(|e| format!("序列化运行记录失败：{e}"))?;
    state
        .host_transport
        .write_file(&run.host, &path, raw.into_bytes())
        .await
        .map_err(|e| format!("写入那台机器上的运行记录失败（{path}）：{e}"))
}

/// Records one instance in **that machine's** running record (ADR-0007 §3).
pub(crate) async fn remember_remote_run(
    state: &AppState,
    instance_id: &str,
    record: &RemoteRunRecord,
    run: &RemoteRun,
) -> Result<(), String> {
    let _guard = REMOTE_RECORD_LOCK.lock().await;
    let mut map = read_remote_records(state, run).await?;
    let value = serde_json::to_value(record).map_err(|e| format!("序列化运行记录失败：{e}"))?;
    map.insert(instance_id.to_string(), value);
    write_remote_records(state, run, &map).await
}

/// Drops one instance from that machine's record: the process is gone, so the
/// token it printed is worthless, and a stale URL would be handed to the next
/// reader as if it were current (the same rule [`forget_url`] follows locally).
///
/// A record the launcher cannot read is left exactly as it is — rewriting it
/// from an empty read would delete every other instance's entry.
pub(crate) async fn forget_remote_run(
    state: &AppState,
    instance_id: &str,
    run: &RemoteRun,
) -> Result<(), String> {
    let _guard = REMOTE_RECORD_LOCK.lock().await;
    let mut map = read_remote_records(state, run).await?;
    if map.remove(instance_id).is_none() {
        return Ok(());
    }
    write_remote_records(state, run, &map).await
}

/// The token of an instance URL, if it carries one. DSH's launch token exists
/// only inside that URL, which is why the record keeps a copy of it: an instance
/// whose token was lost is running but unmanageable.
fn token_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url.trim())
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, value)| value.to_string())
}

/// The instance URL on one line of its output — the same line 本机 reads off the
/// child's stdout, from the same regex.
fn url_in(line: &str) -> Option<String> {
    url_re().captures(line).map(|cap| cap[1].to_string())
}

/// One profile's `package.json`, the document that decides whether the profile
/// is a DSH web application.
fn profile_package_json(home_path: &Path, profile: &str) -> PathBuf {
    crate::profile::profile_dir(home_path, profile)
        .ok()
        .map(|dir| dir.join("package.json"))
        .unwrap_or_else(|| home_path.join("profiles").join(profile).join("package.json"))
}

/// Whether one profile document declares the DSH web application among its
/// bundles. Such a profile understands the web flags and gets a port.
fn web_profile_document(raw: &str) -> bool {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(raw) else {
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

/// The same question asked of the machine that owns the profile: the document
/// travels, the rule does not.
async fn is_web_profile_remote(
    state: &AppState,
    run_host: &crate::ssh::RemoteHost,
    home_path: &Path,
    profile: &str,
) -> bool {
    let pkg = profile_package_json(home_path, profile);
    match state
        .host_transport
        .read_file(run_host, &pkg.to_string_lossy())
        .await
    {
        Ok(bytes) => web_profile_document(&String::from_utf8_lossy(&bytes)),
        Err(_) => false,
    }
}

async fn supports_no_open_remote(
    state: &AppState,
    run_host: &crate::ssh::RemoteHost,
    version_dir: &Path,
) -> bool {
    let startup = crate::launch::web_app_startup_script(version_dir);
    match state
        .host_transport
        .read_file(run_host, &startup.to_string_lossy())
        .await
    {
        Ok(bytes) => crate::launch::supports_no_open(&String::from_utf8_lossy(&bytes)),
        Err(_) => false,
    }
}

/// Re-asserts the launcher's per-instance port in the profile's managed lan-bind
/// block, **on the machine that owns the profile**: that block outranks the CLI
/// `--port` and the plugin only re-asserts it at boot, so without this nudge the
/// first start after a port change comes up on the old port (see
/// [`crate::profile::assert_profile_lan_bind_port`] for the local twin).
///
/// A failure only costs the next start's port, so it is logged rather than
/// raised — the same weight the local one carries.
async fn assert_remote_lan_bind_port(
    state: &AppState,
    run_host: &crate::ssh::RemoteHost,
    home_path: &Path,
    profile: &str,
    port: u16,
) {
    let Some(patch) = crate::profile::lan_bind_patch_path(home_path, profile) else {
        return;
    };
    let path = patch.to_string_lossy().to_string();
    let raw = match crate::remote::read_file_optional(&*state.host_transport, run_host, &path).await
    {
        Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
        // Absent file or absent block: nothing to nudge.
        Ok(None) | Err(_) => return,
    };
    let updated = crate::profile::set_lan_bind_port(&raw, port);
    if updated == raw {
        return;
    }
    if let Err(e) = state
        .host_transport
        .write_file(run_host, &path, updated.into_bytes())
        .await
    {
        crate::log_warn!("固定那台机器上 profile 的端口失败（{path}）: {e}");
    }
}

/// Pins the profile's own pnpm store and peer policy **on that machine**.
///
/// The DSH Web UI's plugin manager runs bare `pnpm` in the profile directory and
/// never sees the launcher's `--store-dir`; the profile itself has to record the
/// settings, or every add/remove there fails with ERR_PNPM_UNEXPECTED_STORE.
/// See [`crate::plugins::ensure_profile_store_pin`] / `ensure_profile_npmrc` for
/// the local twins and why each setting exists. Best effort, like theirs.
async fn pin_remote_profile_store(
    state: &AppState,
    run_host: &crate::ssh::RemoteHost,
    home_path: &Path,
    profile: &str,
    data_dir: &Path,
) {
    let Ok(dir) = crate::profile::profile_dir(home_path, profile) else {
        return;
    };
    let read = |file: PathBuf| async move {
        let path = file.to_string_lossy().to_string();
        match crate::remote::read_file_optional(&*state.host_transport, run_host, &path).await {
            Ok(Some(bytes)) => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
            Ok(None) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    };

    // The store the profile is already linked from, so a profile a user
    // installed against their own store keeps working.
    let modules = dir.join("node_modules").join(".modules.yaml");
    let linked = match read(modules).await {
        Ok(Some(raw)) => crate::plugins::linked_store_document(&raw),
        _ => None,
    };
    let store = crate::plugins::store_target_from(linked.as_deref(), &crate::toolchain::store_dir(data_dir));

    let workspace = dir.join("pnpm-workspace.yaml");
    match read(workspace.clone()).await {
        Ok(raw) => {
            if let Some(out) = crate::plugins::store_pin_document(raw.as_deref(), &store) {
                let path = workspace.to_string_lossy().to_string();
                if let Err(e) = state
                    .host_transport
                    .write_file(run_host, &path, out.into_bytes())
                    .await
                {
                    crate::log_warn!("固定那台机器上 profile 的 pnpm store 失败（{path}）: {e}");
                }
            }
        }
        Err(e) => crate::log_warn!(
            "读取那台机器上的 pnpm-workspace.yaml 失败（{}）: {e}",
            workspace.display()
        ),
    }

    let npmrc = dir.join(".npmrc");
    match read(npmrc.clone()).await {
        Ok(raw) => {
            let raw = raw.unwrap_or_default();
            if let Some(out) = crate::plugins::npmrc_document(&raw) {
                let path = npmrc.to_string_lossy().to_string();
                if let Err(e) = state
                    .host_transport
                    .write_file(run_host, &path, out.into_bytes())
                    .await
                {
                    crate::log_warn!("固定那台机器上 profile 的 peer 策略失败（{path}）: {e}");
                }
            }
        }
        Err(e) => crate::log_warn!(
            "读取那台机器上的 .npmrc 失败（{}）: {e}",
            npmrc.display()
        ),
    }
}

/// What one remote start leaves behind.
#[derive(Clone, Debug)]
pub(crate) struct RemoteStart {
    pub run: RemoteRun,
    /// Where **this run's** output begins in that machine's log.
    pub offset: u64,
}

/// Starts one instance **on a remote host** and returns where it runs.
///
/// The command line and the environment are 本机's policy, taken from
/// [`crate::launch`], with two differences: the answers that depend on the other
/// machine's files (whether the profile is a web application, whether its bundle
/// understands `--no-open`, whether the version's `bin.js` is there) are asked
/// there, and the launcher's proxy settings never ride along — a remote host's
/// network exit is its record's business (`launch::instance_env_for`).
///
/// Everything here is a fact about the other machine: nothing is emitted and
/// nothing is registered, so the caller owns the projections.
pub(crate) async fn spawn_remote_instance(
    state: &AppState,
    cfg: &crate::config::Config,
    host: &Host,
    inst: &DshInstance,
    version: &DshVersion,
    profile: &str,
) -> Result<RemoteStart, String> {
    let run_host = crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;
    let data_dir = host.data_dir(&state.data_dir)?;

    // The version's own `bin.js`, asked of the machine that has the version
    // directory: `version_bin_ready` is a local `exists()`, which at a remote
    // path answers about this Mac.
    let bin = crate::launch::version_bin(&version.dir);
    let ready = crate::remote::file_ready(&*state.host_transport, &run_host, &bin.to_string_lossy())
        .await
        .map_err(|e| format!("无法确认那台机器上的 {}：{e}", bin.display()))?;
    if !ready {
        return Err(format!(
            "版本 {} 安装不完整（缺少 {}），请重新安装",
            version.version,
            bin.display()
        ));
    }

    // Web-app profiles (their bundle list includes @deepseek-ai/dsh-web-app) get
    // a port; the answer comes from that machine's copy of the profile, so a
    // profile whose name is not literally "web" is still recognized — and one
    // that is, but is not a web app there, is not.
    let home_path = cfg
        .homes
        .iter()
        .find(|h| h.id == inst.home_id)
        .map(|h| h.path.clone());
    let is_web = match home_path.as_deref() {
        Some(home) => is_web_profile_remote(state, &run_host, home, profile).await,
        None => profile == "web",
    };
    let web_port = is_web.then(|| inst.port.unwrap_or(0));

    if let (true, Some(home)) = (is_web, home_path.as_deref()) {
        assert_remote_lan_bind_port(state, &run_host, home, profile, inst.port.unwrap_or(0)).await;
    }
    if let Some(home) = home_path.as_deref() {
        pin_remote_profile_store(state, &run_host, home, profile, &data_dir).await;
    }

    // The interpreter is that machine's resolved binding (ADR-0005 追加), and
    // the flags are the launcher's policy — `--host` deliberately never among
    // them, `--no-open` only when that machine's bundle understands it.
    let node = crate::runtime::node_for_spawn_checked_for(state, Some(&host.id)).await?;
    let no_open = web_port.is_some() && supports_no_open_remote(state, &run_host, &version.dir).await;
    let instance_argv =
        crate::launch::instance_argv(&node, &version.dir, profile, web_port, no_open);

    // Environment variables travel as `env` arguments: the detached spawn's own
    // shell gets the command line, and this is what hands the child its
    // environment without a second shell layer to quote through.
    let mut argv = vec!["/usr/bin/env".to_string()];
    for (key, value) in crate::launch::instance_env_for(cfg, &inst.id, host)? {
        argv.push(format!("{key}={value}"));
    }
    argv.extend(instance_argv);

    let log_path = data_dir
        .join("logs")
        .join(format!("{}.log", inst.id))
        .to_string_lossy()
        .to_string();

    // Read the log's size **before** the process exists: the file is appended to
    // across runs, and the previous run's URL line would otherwise be read as
    // this one's.
    let offset = crate::remote::log_size(&*state.host_transport, &run_host, &log_path)
        .await
        .map_err(|e| format!("无法读取那台机器上的日志（{log_path}）：{e}"))?;

    let pid = state
        .host_transport
        .spawn_detached(&run_host, &argv, &log_path)
        .await
        .map_err(|e| format!("在「{}」上启动失败：{e}", host.display_name))?;
    let started_at = crate::remote::process_start_time(&*state.host_transport, &run_host, pid)
        .await
        .ok()
        .flatten();

    let run = RemoteRun {
        host_id: host.id.clone(),
        host: run_host,
        pid,
        started_at,
        data_dir,
        log_path,
    };
    // The record is written before the URL is known and completed when it
    // appears, so a launcher that dies in between still knows which pid it left
    // running where.
    let record = RemoteRunRecord {
        url: None,
        token: None,
        profile: profile.to_string(),
        pid,
        started_at,
    };
    if let Err(e) = remember_remote_run(state, &inst.id, &record, &run).await {
        crate::log_warn!(
            "实例 {} 的远端运行记录没写上（{e}）；进程已在 pid {pid} 上跑",
            inst.id
        );
    }

    Ok(RemoteStart { run, offset })
}

/// What one read of a remote instance's log says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteLogRead {
    /// Where to read from next.
    pub offset: u64,
    /// The instance's URL, once its output has printed one.
    pub url: Option<String>,
    /// Whether the process is still there.
    pub alive: bool,
}

/// Reads one remote instance's log and, on the way, the two things the launcher
/// needs out of it: the URL it printed (taken down into that machine's running
/// record, token and all) and whether the process is still there.
///
/// `pending` carries whatever the last read ended in the middle of: a chunk is a
/// byte range, so a line — the URL line above all — can be split across two
/// reads, and half a line is not something to act on.
pub(crate) async fn read_remote_log(
    state: &AppState,
    instance_id: &str,
    profile: &str,
    run: &RemoteRun,
    offset: u64,
    pending: &mut String,
) -> Result<RemoteLogRead, String> {
    let chunk = crate::remote::tail(
        &*state.host_transport,
        &run.host,
        &run.log_path,
        offset,
        run.pid,
        REMOTE_TAIL_WAIT_SECS,
    )
    .await
    .map_err(|e| format!("读取那台机器上的日志失败（{}）：{e}", run.log_path))?;

    pending.push_str(&chunk.text);
    let mut url = None;
    while let Some(cut) = pending.find('\n') {
        let line = pending[..cut].trim_end_matches('\r').to_string();
        pending.drain(..=cut);
        if url.is_none() {
            url = url_in(&line);
        }
    }

    if let Some(found) = url.as_deref() {
        let record = RemoteRunRecord {
            url: Some(found.to_string()),
            token: token_of(found),
            profile: profile.to_string(),
            pid: run.pid,
            started_at: run.started_at,
        };
        if let Err(e) = remember_remote_run(state, instance_id, &record, run).await {
            // The instance is up either way; losing the record only costs the
            // next session the token, so it is loud in the log, not fatal here.
            crate::log_warn!("实例 {instance_id} 的远端运行记录没写上（{e}）");
        }
    }

    Ok(RemoteLogRead {
        offset: chunk.offset,
        url,
        alive: chunk.alive,
    })
}

/// Starts the remote half of a start and hands the rest to the follower.
async fn start_remote_instance(
    app: &AppHandle,
    state: &State<'_, AppState>,
    cfg: &crate::config::Config,
    host: &Host,
    inst: &DshInstance,
    version: &DshVersion,
    profile: &str,
) -> Result<(), String> {
    emit_status(
        app,
        &InstanceStatus {
            id: inst.id.clone(),
            state: InstanceState::Starting,
            url: None,
            profile: Some(profile.to_string()),
            exit_code: None,
        },
    );

    let start = match spawn_remote_instance(state, cfg, host, inst, version, profile).await {
        Ok(start) => start,
        Err(e) => {
            // Nothing is running: say so instead of leaving the card on 启动中.
            emit_status(
                app,
                &InstanceStatus {
                    id: inst.id.clone(),
                    state: InstanceState::Stopped,
                    url: None,
                    profile: Some(profile.to_string()),
                    exit_code: None,
                },
            );
            return Err(e);
        }
    };

    crate::log_info!(
        "实例 {} 已在「{}」上拉起（pid {}，profile: {profile}）",
        inst.id,
        host.display_name,
        start.run.pid
    );

    register_running(
        app,
        state,
        &inst.id,
        RunningInstance {
            kill: Arc::new(Notify::new()),
            profile: profile.to_string(),
            url: None,
            adopted: None,
            remote: Some(start.run.clone()),
        },
    )
    .await;

    follow_remote_instance(app, &inst.id, profile, start.run, start.offset);
    Ok(())
}

/// Watches one remote instance's log until the process is gone: reports the URL
/// it printed (which is what makes the card say 运行 and the plugins page able to
/// authenticate) and cleans up when it exits.
///
/// The read waits on the far side, so this loop costs one ssh per quiet window
/// rather than one per second — the same shape the install follower uses.
fn follow_remote_instance(
    app: &AppHandle,
    instance_id: &str,
    profile: &str,
    run: RemoteRun,
    offset: u64,
) {
    let app = app.clone();
    let id = instance_id.to_string();
    let profile = profile.to_string();
    tauri::async_runtime::spawn(async move {
        let mut offset = offset;
        let mut pending = String::new();
        let mut announced = false;
        loop {
            let state = app.state::<AppState>();
            // Stop coordinating when the entry is gone or was replaced: the user
            // stopped the instance, or a start raced this follower.
            let still_ours = state
                .running
                .lock()
                .await
                .get(&id)
                .and_then(|entry| entry.remote.as_ref())
                .map(|entry| entry.pid == run.pid)
                .unwrap_or(false);
            if !still_ours {
                return;
            }

            let read = read_remote_log(&state, &id, &profile, &run, offset, &mut pending).await;
            let read = match read {
                Ok(read) => read,
                Err(e) => {
                    // The machine could not answer — the process may still be
                    // running there, so this is not an exit.
                    crate::log_warn!("实例 {id} 的远端日志暂时读不到：{e}");
                    tokio::time::sleep(REMOTE_FOLLOW_RETRY).await;
                    continue;
                }
            };
            offset = read.offset;

            if let Some(url) = read.url.filter(|_| !announced) {
                announced = true;
                crate::log_info!("实例 {id} 已就绪：{url}");
                {
                    let mut running = state.running.lock().await;
                    if let Some(entry) = running.get_mut(&id) {
                        entry.url = Some(url.clone());
                    }
                }
                emit_status(
                    &app,
                    &InstanceStatus {
                        id: id.clone(),
                        state: InstanceState::Running,
                        url: Some(url),
                        profile: Some(profile.clone()),
                        exit_code: None,
                    },
                );
            }

            if !read.alive {
                crate::log_info!("实例 {id} 在那台机器上已退出（pid {}）", run.pid);
                unregister_running(
                    &app,
                    &state,
                    &id,
                    InstanceStatus {
                        id: id.clone(),
                        state: InstanceState::Exited,
                        url: None,
                        profile: Some(profile.clone()),
                        exit_code: None,
                    },
                )
                .await;
                return;
            }
        }
    });
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

    // A remote instance is started **there** (ADR-0007): the version tree, the
    // profile and the process all belong to that machine, so everything below —
    // which reads this Mac's filesystem and spawns this Mac's child processes —
    // is the wrong half of the world for it.
    let host = crate::config::resolve_host(&cfg, Some(&inst.host_id))?.clone();
    if !host.builtin {
        {
            let running = state.running.lock().await;
            if running.contains_key(instance_id) {
                return Err("实例已在运行".to_string());
            }
        }
        return start_remote_instance(app, state, &cfg, &host, &inst, version, profile).await;
    }

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

    // The running instance's own plugin manager (the Web UI's 插件 page) spawns
    // bare `pnpm` in the profile directory — it never sees the `--store-dir`
    // the launcher passes on its own invocations, so it falls back to the
    // user's global store and pnpm refuses every add/remove with
    // ERR_PNPM_UNEXPECTED_STORE. Pinning the store in the profile itself is
    // what that process reads; a failure here only costs the Web UI's plugin
    // management, so the launch continues.
    if let Some(hp) = home_path.as_deref() {
        let launcher_store = crate::toolchain::store_dir(&state.data_dir);
        match crate::profile::profile_dir(hp, profile) {
            Ok(dir) => {
                // The store the profile is already linked from, so a profile a
                // user installed by hand against their global store keeps
                // working instead of being dragged onto the launcher's.
                let store = crate::plugins::profile_store_target(&dir, &launcher_store);
                if let Err(e) = crate::plugins::ensure_profile_store_pin(&dir, &store) {
                    crate::log_warn!("固定 profile pnpm store 失败（{}）: {e}", dir.display());
                }
                // The same bare pnpm installs into the profile: with
                // auto-install-peers on, a plugin whose peers include a core
                // package would drag a duplicate copy of it in (doctor reports
                // the duplicated-Symbol failure). Pin the peer policy too, so
                // the Web UI's own installs stay independent of the user's
                // global pnpm config.
                if let Err(e) = crate::plugins::ensure_profile_npmrc(&dir) {
                    crate::log_warn!("固定 profile peer 策略失败（{}）: {e}", dir.display());
                }
            }
            Err(e) => crate::log_warn!("解析 profile 目录失败，跳过 store 固定: {e}"),
        }
    }

    // The interpreter is the resolved binding rather than a bare name, and a
    // toolchain installed while the app was running is picked up here (the
    // binding is re-probed when its recorded path no longer exists).
    let node = crate::runtime::node_for_spawn_checked(state).await?;

    // The launch spec module owns the argv: --profile, per-instance --port,
    // --no-open feature detection, and the deliberate absence of --host (the
    // profile layer owns the bind host).
    let mut cmd = crate::launch::instance_command(
        &node,
        &version.dir,
        profile,
        is_web.then(|| inst.port.unwrap_or(0)),
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
            remote: None,
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
                    // Keep the URL (token included) across launcher restarts:
                    // see `remember_url`.
                    remember_url(&state.data_dir, &reader_id, &url, &reader_profile);
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
        .map(|r| (r.kill.clone(), r.adopted.clone(), r.remote.clone()));
    let Some((kill, adopted, remote)) = entry else {
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
    if let Some(run) = remote {
        // A remote instance is stopped on the machine that runs it, by pid, with
        // the recorded process start time checked first (ADR-0007 §4) — that
        // belongs to the stop/restart/adoption pass, and until it exists this
        // refuses rather than falling into the poll below, which has no child to
        // wait for and would end in a timeout the user cannot act on.
        return Err(format!(
            "远端实例的停止还没有实现：它跑在「{}」上（pid {}），现在只能在那台机器上停它",
            run.host.target, run.pid
        ));
    }
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
///
/// **Remote instances are not signalled** (ADR-0007 §1-2): they run on someone
/// else's machine in their own session, and outliving this launcher is the whole
/// point of putting them there. ⌘Q leaves them alone; the next session adopts
/// them.
pub fn kill_all(state: &AppState) {
    let running = state.running.blocking_lock();
    for entry in running.values() {
        if entry.remote.is_some() {
            continue;
        }
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

/// What the running table says about one instance, as needed by callers that
/// only want to know where an instance lives and whether it can be managed.
pub struct RunningEntry {
    /// The instance this entry belongs to. Callers that were handed an
    /// instance id report it back, so a page can say which instance actually
    /// answered instead of assuming the one it was opened from.
    pub id: String,
    /// The instance URL; when it carries the one-time start token
    /// (`...?token=...`) the launcher can authenticate, otherwise it is bare
    /// (`http://host:port`) and only good for opening in a browser.
    pub url: Option<String>,
    /// Adopted from an external process rather than spawned by this launcher
    /// session. Adoption alone no longer means "unmanageable" — that is
    /// decided by [`manageable_url`] — but it is what tells the caller to stop
    /// it by pid instead of by handle.
    pub adopted: bool,
}

/// Resolves the running instance that is executing `profile` of `home_id`:
/// the same DSH_HOME and the running profile equal to `profile`. `prefer` names
/// the instance the caller came from — when it is itself running that profile
/// it wins, and the first registered match is only the fallback (design §4).
///
/// The preference exists because one DSH_HOME and one profile can be running in
/// two instances at once: the Plugins page must talk to the instance whose row
/// the user clicked, or a toggle would be applied by a process they are not
/// looking at.
///
/// Returns `Err` only when the DSH_HOME itself is unknown — "no running
/// instance" is `Ok(None)`, which is a normal state (ADR-0001), not an error.
pub async fn running_for_profile(
    state: &AppState,
    home_id: &str,
    profile: &str,
    prefer: Option<&str>,
) -> Result<Option<RunningEntry>, String> {
    let instance_ids: Vec<String> = {
        let cfg = state.config.lock().unwrap();
        if !cfg.homes.iter().any(|h| h.id == home_id) {
            return Err("DSH_HOME 不存在".to_string());
        }
        // Instances that belong to this HOME, in registration order.
        cfg.instances
            .iter()
            .filter(|i| i.home_id == home_id)
            .map(|i| i.id.clone())
            .collect()
    };
    let running = state.running.lock().await;
    let order = match prefer {
        // The named instance first, the rest in registration order. An id that
        // belongs to another HOME (or to no instance) is ignored: the caller
        // can only be pointing at an instance of the HOME it asked about.
        Some(preferred) if instance_ids.iter().any(|id| id == preferred) => {
            let mut order = vec![preferred.to_string()];
            order.extend(instance_ids.iter().filter(|id| id.as_str() != preferred).cloned());
            order
        }
        _ => instance_ids,
    };
    for id in order {
        if let Some(entry) = running.get(&id) {
            if entry.profile == profile {
                return Ok(Some(RunningEntry {
                    id,
                    url: entry.url.clone(),
                    adopted: entry.adopted.is_some(),
                }));
            }
        }
    }
    Ok(None)
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
    let remote = state.running.lock().await.remove(id).and_then(|r| r.remote);
    // The process is gone, so the token it printed is worthless: drop the
    // reminder rather than handing a stale URL to the next adoption.
    forget_url(&state.data_dir, id);
    // Same on the machine that ran it: its own record is where a later session
    // would find the pid and the token, and neither outlives the process.
    if let Some(run) = remote {
        if let Err(e) = forget_remote_run(state, id, &run).await {
            crate::log_warn!("实例 {id} 的远端运行记录没能清掉：{e}");
        }
    }
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

    fn app_state(
        homes: Vec<crate::config::DshHome>,
        instances: Vec<crate::config::DshInstance>,
    ) -> AppState {
        AppState {
            config_path: std::path::PathBuf::new(),
            data_dir: std::path::PathBuf::new(),
            config: std::sync::Mutex::new(crate::config::Config {
                homes,
                versions: Vec::new(),
                instances,
                settings: Default::default(),
                hosts: vec![crate::config::Host::local()],
            }),
            running: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            tasks: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            // The local lifecycle never talks to a remote host; an in-memory
            // transport that answers nothing is the honest stand-in.
            host_transport: crate::remote::testing::idle(),
            host_statuses: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    // -----------------------------------------------------------------------
    // Remote instances
    //
    // These drive the remote half of a start through the transport double: what
    // the launcher *asks that machine to do* (the detached spawn, the record it
    // writes) and what it reads back out of that machine's log are the whole
    // contract, so the tests assert on those and not on which of the launcher's
    // own functions ran. The Tauri side of a start — the status event and the
    // tray rebuild — has no double, and is not driven here.
    // -----------------------------------------------------------------------

    use crate::remote::testing::{marked, FakeAnswer, FakeCall, FakeTransport};

    const REMOTE_DATA: &str = "/home/demo/.local/share/dsh-launcher";
    const REMOTE_HOME: &str = "/home/demo/.dsh";

    fn remote_host(id: &str) -> crate::config::Host {
        crate::config::Host {
            id: id.to_string(),
            display_name: "阿里云服务器".into(),
            ssh_target: "zjq_aliyun".into(),
            registry: None,
            extra_env: [("HTTPS_PROXY".to_string(), "http://10.0.0.9:3128".to_string())]
                .into_iter()
                .collect(),
            data_dir: Some(PathBuf::from(REMOTE_DATA)),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    fn remote_home(host_id: &str) -> crate::config::DshHome {
        crate::config::DshHome {
            id: "h-remote".into(),
            name: "服务器默认".into(),
            path: PathBuf::from(REMOTE_HOME),
            host_id: host_id.to_string(),
        }
    }

    fn remote_version(host_id: &str) -> crate::config::DshVersion {
        crate::config::DshVersion {
            id: "v-remote".into(),
            version: "0.1.0-rc.6".into(),
            dir: PathBuf::from(REMOTE_DATA).join("versions").join("0.1.0-rc.6"),
            host_id: host_id.to_string(),
        }
    }

    fn remote_instance(host_id: &str, port: Option<u16>) -> crate::config::DshInstance {
        crate::config::DshInstance {
            id: "i-remote".into(),
            name: "服务器实例".into(),
            version_id: "v-remote".into(),
            home_id: "h-remote".into(),
            host_id: host_id.to_string(),
            env_overrides: [("DSH_TELEMETRY_DISABLED".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
            default_profile: Some("web".into()),
            last_profile: None,
            icon: None,
            port,
        }
    }

    /// An `AppState` for one remote host, with the launcher's own proxy settings
    /// switched on — they belong to this Mac and must not reach that machine.
    fn remote_app_state(
        fake: std::sync::Arc<FakeTransport>,
        host: crate::config::Host,
        instance: crate::config::DshInstance,
    ) -> AppState {
        AppState {
            config_path: std::env::temp_dir()
                .join(format!("dsh-proc-test-{}", uuid::Uuid::new_v4()))
                .join("config.json"),
            data_dir: PathBuf::from("/launcher"),
            config: std::sync::Mutex::new(crate::config::Config {
                homes: vec![remote_home(&host.id)],
                versions: vec![remote_version(&host.id)],
                instances: vec![instance],
                settings: crate::config::LauncherSettings {
                    proxy_enabled: true,
                    proxy_apply_dsh: true,
                    proxy_url: "http://127.0.0.1".into(),
                    proxy_port: 7890,
                    ..Default::default()
                },
                hosts: vec![crate::config::Host::local(), host],
            }),
            running: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            tasks: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            host_transport: fake,
            host_statuses: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn web_profile_document_fixture() -> Vec<u8> {
        br#"{"name":"dsh-profile-web","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app"]}}}"#.to_vec()
    }

    /// A start's answers, in the order the launcher asks them: the two profile
    /// documents it reads from that machine, the port nudge (no patch document
    /// yet), the profile's own store and peer pins, that machine's toolchain,
    /// the `--no-open` feature check, the log size, the spawn, the pid's start
    /// time, and the empty running record it completes.
    fn remote_start_answers(node: &str) -> Vec<FakeAnswer> {
        vec![
            // 1. 版本目录里的 bin.js 在不在（那台机器自己回答）。
            FakeAnswer::stdout(marked(r#"{"ready":true}"#)),
            // 2. 那个 profile 的 package.json：是不是 web 应用。
            FakeAnswer::Bytes(web_profile_document_fixture()),
            // 3. profile 的 lan-bind 文档：还没有，没有要改的行。
            FakeAnswer::failed(2, "远端没有这个文件：cordis.patch.yml"),
            // 4.-6. profile 自己那份 pnpm store：先看它已链接到哪个 store，
            //       再把 store 写进 pnpm-workspace.yaml。
            FakeAnswer::failed(2, "远端没有这个文件：node_modules/.modules.yaml"),
            FakeAnswer::failed(2, "远端没有这个文件：pnpm-workspace.yaml"),
            FakeAnswer::Done,
            // 7.-8. 同一份 profile 的 peer 策略。
            FakeAnswer::failed(2, "远端没有这个文件：.npmrc"),
            FakeAnswer::Done,
            // 9. 那台机器自己的绝对 Node。
            FakeAnswer::stdout(crate::remote::testing::environment_stdout(
                crate::remote::testing::remote_tool(node, "v26.0.0", "system"),
                crate::remote::testing::remote_tool_missing(vec![]),
                &[],
            )),
            // 10. 那台机器的 bundle 认不认 --no-open。
            FakeAnswer::Bytes(b"const p = new Command().option('--no-open', 'x')".to_vec()),
            // 11. 日志现在有多少字节（起进程之前问的）。
            FakeAnswer::stdout(marked(r#"{"offset":0}"#)),
            // 12. 脱离会话地起进程。
            FakeAnswer::Pid(4242),
            // 13. 这个 pid 自己的启动时间。
            FakeAnswer::stdout(marked(r#"{"started_at":117384720}"#)),
            // 14.-15. 那台机器上的运行记录：还没有，写一条。
            FakeAnswer::failed(2, "远端没有这个文件：running.json"),
            FakeAnswer::Done,
        ]
    }

    fn last_write_of(fake: &FakeTransport, path: &str) -> Vec<u8> {
        fake.calls()
            .into_iter()
            .rev()
            .find_map(|call| match call {
                FakeCall::WriteFile(written, bytes) if written == path => Some(bytes),
                _ => None,
            })
            .unwrap_or_else(|| panic!("没有写下 {path}"))
    }

    /// The URL DSH prints on readiness, and the line it arrives on.
    const READY_URL: &str = "http://127.0.0.1:3080/?token=JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";
    const READY_TOKEN: &str = "JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";

    fn remote_run() -> RemoteRun {
        RemoteRun {
            host_id: "host-server".into(),
            host: crate::ssh::RemoteHost {
                id: "host-server".into(),
                target: "zjq_aliyun".into(),
                socket: PathBuf::from("/launcher/ssh/abc.sock"),
            },
            pid: 4242,
            started_at: Some(117384720),
            data_dir: PathBuf::from(REMOTE_DATA),
            log_path: format!("{REMOTE_DATA}/logs/i-remote.log"),
        }
    }

    /// The whole point of ADR-0007's detached spawn: the instance is started by
    /// that machine's own shell in its own session — `setsid`, stdin from
    /// `/dev/null`, output appended to that machine's log — with the absolute
    /// Node its own probe resolved, the version's `bin.js`, and the environment
    /// the launcher promised (DSH_HOME, the instance's name and own variables,
    /// the host record's variables) and **not** this Mac's proxy settings.
    #[tokio::test]
    async fn a_remote_start_spawns_detached_with_that_hosts_toolchain_and_environment() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let node = "/home/linuxbrew/.linuxbrew/bin/node";
        let fake = FakeTransport::new();
        for answer in remote_start_answers(node) {
            fake.answering(answer);
        }
        let state = remote_app_state(fake.clone(), remote_host(&host_id), remote_instance(&host_id, Some(3080)));
        let cfg = state.config.lock().unwrap().clone();
        let host = cfg.hosts.iter().find(|h| h.id == host_id).unwrap().clone();
        let inst = cfg.instances[0].clone();
        let version = cfg.versions[0].clone();

        let start = spawn_remote_instance(&state, &cfg, &host, &inst, &version, "web")
            .await
            .unwrap();

        let script = fake
            .calls()
            .into_iter()
            .find_map(|call| match call {
                FakeCall::Spawn(script) => Some(script),
                _ => None,
            })
            .expect("远端启动要起一个脱离会话的进程");
        // Detached: its own session, no stdin, output appended to that machine's
        // log — which is what lets it outlive the ssh connection and this app.
        assert!(script.contains("nohup setsid"), "{script}");
        assert!(script.contains("< /dev/null"), "{script}");
        assert!(
            script.contains(&format!("dsh_log='{REMOTE_DATA}/logs/i-remote.log'"))
                && script.contains("2>&1 < /dev/null"),
            "输出追加到那台机器上的日志：{script}"
        );
        // Absolute Node and bin.js from that machine's own version directory.
        assert!(script.contains(&format!("'{node}'")), "{script}");
        assert!(
            script.contains(&format!(
                "'{REMOTE_DATA}/versions/0.1.0-rc.6/node_modules/@deepseek-ai/dsh/lib/bin.js'"
            )),
            "{script}"
        );
        assert!(script.contains("'--profile' 'web'"), "{script}");
        assert!(script.contains("'--port' '3080'"), "{script}");
        assert!(script.contains("'--no-open'"), "那台机器的 bundle 认这个 flag：{script}");
        assert!(!script.contains("--host"), "绑定地址归 profile 层：{script}");
        // The environment: DSH_HOME, the instance's name, its own variables, and
        // the host record's variables.
        assert!(script.contains(&format!("'DSH_HOME={REMOTE_HOME}'")), "{script}");
        assert!(script.contains("'DSH_LAUNCHER_INSTANCE=服务器实例'"), "{script}");
        assert!(script.contains("'DSH_TELEMETRY_DISABLED=1'"), "{script}");
        assert!(script.contains("'HTTPS_PROXY=http://10.0.0.9:3128'"), "远端的出口由主机记录决定：{script}");
        assert!(
            !script.contains("127.0.0.1:7890"),
            "本机代理只作用于本机实例：{script}"
        );

        assert_eq!(start.run.pid, 4242);
        assert_eq!(start.run.host_id, host_id);
        assert_eq!(start.run.data_dir, PathBuf::from(REMOTE_DATA));
        assert_eq!(start.run.started_at, Some(117384720));
        assert_eq!(start.offset, 0, "这次运行的输出从启动那一刻的日志末尾开始");
    }

    /// ADR-0007 §3: the record that machine keeps names the pid **and** its
    /// process start time — a pid number alone is reused by the kernel — with the
    /// profile, and no URL until the instance prints one. It is written before
    /// the URL appears so a launcher that dies in between still knows what it
    /// started where.
    #[tokio::test]
    async fn the_remote_run_record_names_the_pid_its_start_time_and_the_profile() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let fake = FakeTransport::new();
        for answer in remote_start_answers("/usr/bin/node") {
            fake.answering(answer);
        }
        let state = remote_app_state(fake.clone(), remote_host(&host_id), remote_instance(&host_id, Some(3080)));
        let cfg = state.config.lock().unwrap().clone();
        let host = cfg.hosts.iter().find(|h| h.id == host_id).unwrap().clone();
        let inst = cfg.instances[0].clone();
        let version = cfg.versions[0].clone();

        spawn_remote_instance(&state, &cfg, &host, &inst, &version, "web")
            .await
            .unwrap();

        let path = format!("{REMOTE_DATA}/running.json");
        let written = last_write_of(&fake, &path);
        let doc: serde_json::Value = serde_json::from_slice(&written).unwrap();
        let entry = &doc["i-remote"];
        assert_eq!(entry["pid"], 4242, "{doc}");
        assert_eq!(entry["started_at"], 117384720, "{doc}");
        assert_eq!(entry["profile"], "web", "{doc}");
        assert_eq!(entry["url"], serde_json::Value::Null, "{doc}");
    }

    /// The URL is where the launch token lives, so reading it out of that
    /// machine's log and writing it — token and all — into that machine's record
    /// is what keeps the instance manageable after a launcher restart.
    #[tokio::test]
    async fn the_url_and_token_come_out_of_that_machines_log_into_its_record() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(crate::remote::testing::tail_stdout(
                &format!("dsh web: {READY_URL}\n"),
                999,
            )))
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(FakeAnswer::Done);
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        let run = remote_run();
        let mut pending = String::new();

        let read = read_remote_log(&state, "i-remote", "web", &run, 0, &mut pending)
            .await
            .unwrap();

        assert_eq!(read.url.as_deref(), Some(READY_URL));
        assert_eq!(read.offset, 999);
        assert!(read.alive);

        let written = last_write_of(&fake, &format!("{REMOTE_DATA}/running.json"));
        let entry = &serde_json::from_slice::<serde_json::Value>(&written).unwrap()["i-remote"];
        assert_eq!(entry["url"], READY_URL, "{entry}");
        assert_eq!(
            entry["token"],
            READY_TOKEN,
            "token 必须落到记录里：{entry}"
        );
        assert_eq!(entry["pid"], 4242, "{entry}");
        assert_eq!(entry["started_at"], 117384720, "{entry}");
        assert_eq!(entry["profile"], "web", "{entry}");
    }

    /// One read is a byte range, so the URL line can arrive in two halves. Acting
    /// on half a line is not an option, and neither is losing it: the URL is what
    /// makes the instance manageable at all.
    #[tokio::test]
    async fn a_url_split_across_two_reads_is_read_once_the_line_is_complete() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(crate::remote::testing::tail_stdout(
                "dsh web: http://127.0.0.1:30",
                40,
            )))
            .answering(FakeAnswer::stdout(crate::remote::testing::tail_stdout(
                "80/?token=abc123\n",
                60,
            )))
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(FakeAnswer::Done);
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        let run = remote_run();
        let mut pending = String::new();

        let first = read_remote_log(&state, "i-remote", "web", &run, 0, &mut pending)
            .await
            .unwrap();
        assert_eq!(first.url, None, "半行 URL 不算数");

        let second = read_remote_log(&state, "i-remote", "web", &run, first.offset, &mut pending)
            .await
            .unwrap();
        assert_eq!(second.url.as_deref(), Some("http://127.0.0.1:3080/?token=abc123"));
    }

    /// A DSH that exited on that machine (a crash, a bad profile) is not the same
    /// as one the launcher cannot see: the read says which it is, and the
    /// follower ends the watch on the former.
    #[tokio::test]
    async fn a_read_of_a_log_whose_process_is_gone_says_so() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(
            crate::remote::testing::tail_stdout_after_exit("Cannot find module 'x'\n", 25),
        ));
        let state = AppState {
            host_transport: fake,
            ..app_state(Vec::new(), Vec::new())
        };
        let run = remote_run();
        let mut pending = String::new();

        let read = read_remote_log(&state, "i-remote", "web", &run, 0, &mut pending)
            .await
            .unwrap();

        assert!(!read.alive);
        assert_eq!(read.url, None);
    }

    /// 本机's token reminder is dropped when the process is gone, so a stale URL
    /// is never handed to the next adoption. A remote instance's record has to
    /// lose its entry the same way — the token it holds points at a process that
    /// no longer exists.
    #[tokio::test]
    async fn forgetting_a_remote_run_removes_only_that_entry() {
        let existing = serde_json::json!({
            "i-other": {"url": "http://127.0.0.1:1/?token=keep", "profile": "web", "pid": 7},
            "i-remote": {"url": null, "profile": "web", "pid": 4242, "started_at": 1},
        })
        .to_string();
        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(crate::remote::testing::file_stdout(
                existing.as_bytes(),
            )))
            .answering(FakeAnswer::Done);
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };

        forget_remote_run(&state, "i-remote", &remote_run()).await.unwrap();

        let written = last_write_of(&fake, &format!("{REMOTE_DATA}/running.json"));
        let doc: serde_json::Value = serde_json::from_slice(&written).unwrap();
        assert!(doc.get("i-remote").is_none(), "{doc}");
        assert_eq!(doc["i-other"]["url"], "http://127.0.0.1:1/?token=keep", "{doc}");
    }

    /// ⌘Q stops this Mac's instances and nothing else (ADR-0007 §1-2): a remote
    /// instance is meant to still be running on that machine the next time the
    /// launcher starts.
    #[test]
    fn launcher_exit_does_not_signal_remote_instances() {
        let state = app_state(Vec::new(), Vec::new());
        let local_kill = Arc::new(Notify::new());
        let remote_kill = Arc::new(Notify::new());
        state.running.blocking_lock().insert(
            "i-local".into(),
            RunningInstance {
                kill: local_kill.clone(),
                profile: "web".into(),
                url: None,
                adopted: None,
                remote: None,
            },
        );
        state.running.blocking_lock().insert(
            "i-remote".into(),
            RunningInstance {
                kill: remote_kill.clone(),
                profile: "web".into(),
                url: None,
                adopted: None,
                remote: Some(remote_run()),
            },
        );

        kill_all(&state);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(300), local_kill.notified())
                .await
                .expect("本机的实例退出启动器时要停掉");
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(300), remote_kill.notified())
                    .await
                    .is_err(),
                "远端实例不该在退出启动器时收到信号"
            );
        });
    }

    fn instance(id: &str, home_id: &str) -> crate::config::DshInstance {
        crate::config::DshInstance {
            id: id.to_string(),
            name: id.to_string(),
            version_id: "v1".to_string(),
            home_id: home_id.to_string(),
            host_id: crate::config::LOCAL_HOST_ID.to_string(),
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        }
    }

    fn home(id: &str) -> crate::config::DshHome {
        crate::config::DshHome {
            id: id.to_string(),
            name: id.to_string(),
            path: std::path::PathBuf::from("/tmp/home"),
            host_id: crate::config::LOCAL_HOST_ID.to_string(),
        }
    }

    fn running_entry(profile: &str, url: Option<&str>) -> RunningInstance {
        RunningInstance {
            kill: Arc::new(Notify::new()),
            profile: profile.to_string(),
            url: url.map(str::to_string),
            adopted: None,
            remote: None,
        }
    }

    /// The URL an instance prints is the only thing that lets a *later*
    /// launcher session manage a process it did not spawn, so it has to
    /// survive the session — and be dropped when the process is gone.
    #[test]
    fn remembered_urls_round_trip_and_forget() {
        let dir = std::env::temp_dir().join(format!("dsh-remembered-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(remembered_path(&dir));

        assert_eq!(recall_url(&dir, "i1"), None);
        remember_url(&dir, "i1", "http://127.0.0.1:3088/?token=abc", "web");
        remember_url(&dir, "i2", "http://127.0.0.1:3089/?token=def", "web");
        assert_eq!(
            recall_url(&dir, "i1").as_deref(),
            Some("http://127.0.0.1:3088/?token=abc")
        );
        // A restart re-reads the same file: same answer, no state in memory.
        assert_eq!(
            recall_url(&dir, "i2").as_deref(),
            Some("http://127.0.0.1:3089/?token=def")
        );

        forget_url(&dir, "i1");
        assert_eq!(recall_url(&dir, "i1"), None);
        assert!(recall_url(&dir, "i2").is_some(), "forget must be per instance");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manageability is the token's business, not the entry's origin.
    #[test]
    fn manageable_url_requires_a_token() {
        let spawned = RunningEntry {
            id: "i1".to_string(),
            url: Some("http://127.0.0.1:1/?token=a".to_string()),
            adopted: false,
        };
        assert!(manageable_url(&spawned).is_some());

        // Adopted, but we remembered the token it printed: manageable.
        let adopted_with_token = RunningEntry {
            id: "i1".to_string(),
            url: Some("http://127.0.0.1:1/?token=a".to_string()),
            adopted: true,
        };
        assert!(manageable_url(&adopted_with_token).is_some());

        let adopted_bare = RunningEntry {
            id: "i1".to_string(),
            url: Some("http://127.0.0.1:1".to_string()),
            adopted: true,
        };
        assert!(manageable_url(&adopted_bare).is_none());

        let unknown = RunningEntry {
            id: "i2".to_string(),
            url: None,
            adopted: false,
        };
        assert!(manageable_url(&unknown).is_none());
    }

    /// Without a preference the Plugins page resolves (HOME, Profile) to the
    /// instance actually running that profile: same HOME, equal profile, and
    /// the first registered match wins.
    #[tokio::test]
    async fn running_for_profile_matches_home_and_profile() {
        let state = app_state(
            vec![home("h1"), home("h2")],
            vec![
                instance("i1", "h1"),
                instance("i2", "h1"),
                instance("i3", "h2"),
            ],
        );
        {
            let mut running = state.running.lock().await;
            // i1 runs a different profile; i2 and i3 run "web".
            running.insert(
                "i1".to_string(),
                running_entry("bot", Some("http://127.0.0.1:1/?token=a")),
            );
            running.insert(
                "i2".to_string(),
                running_entry("web", Some("http://127.0.0.1:2/?token=b")),
            );
            running.insert(
                "i3".to_string(),
                running_entry("web", Some("http://127.0.0.1:3/?token=c")),
            );
        }

        let hit = running_for_profile(&state, "h1", "web", None).await.unwrap().unwrap();
        assert_eq!(hit.url.as_deref(), Some("http://127.0.0.1:2/?token=b"));
        assert!(!hit.adopted);

        // A different HOME never borrows another HOME's instance.
        let other = running_for_profile(&state, "h2", "web", None).await.unwrap().unwrap();
        assert_eq!(other.url.as_deref(), Some("http://127.0.0.1:3/?token=c"));

        // Instance registered but not started -> no running entry.
        assert!(running_for_profile(&state, "h1", "demo", None).await.unwrap().is_none());
        // Unknown HOME is an error, distinct from "not running".
        assert!(running_for_profile(&state, "nope", "web", None).await.is_err());
    }

    /// Two instances can run one DSH_HOME and one profile at the same time. The
    /// page was opened from one of their rows, and the write has to be applied
    /// by that process — the first match is only what happens without one.
    #[tokio::test]
    async fn running_for_profile_prefers_the_instance_the_user_came_from() {
        let state = app_state(
            vec![home("h1"), home("h2")],
            vec![
                instance("i1", "h1"),
                instance("i2", "h1"),
                instance("i9", "h2"),
            ],
        );
        {
            let mut running = state.running.lock().await;
            running.insert("i1".to_string(), running_entry("web", Some("http://127.0.0.1:1/?token=a")));
            running.insert("i2".to_string(), running_entry("web", Some("http://127.0.0.1:2/?token=b")));
            running.insert("i9".to_string(), running_entry("web", Some("http://127.0.0.1:3/?token=c")));
        }

        // Without a preference: registration order, as before.
        let first = running_for_profile(&state, "h1", "web", None).await.unwrap().unwrap();
        assert_eq!(first.id, "i1");

        // With one: the named instance answers, and it says which one it is.
        let preferred = running_for_profile(&state, "h1", "web", Some("i2")).await.unwrap().unwrap();
        assert_eq!(preferred.id, "i2");
        assert_eq!(preferred.url.as_deref(), Some("http://127.0.0.1:2/?token=b"));

        // A stale id (the instance was deleted), and one belonging to another
        // HOME, only rank: the page is scoped by (HOME, Profile), so neither
        // can borrow another HOME's process or turn into an error.
        for ignored in ["nope", "i9"] {
            let hit = running_for_profile(&state, "h1", "web", Some(ignored)).await.unwrap().unwrap();
            assert_eq!(hit.id, "i1", "{ignored} must not change the answer");
        }
    }

    /// An adopted instance has a bare URL and no token: it must surface as
    /// adopted so the caller can refuse to manage it.
    #[tokio::test]
    async fn running_for_profile_reports_adopted_instances() {
        let state = app_state(vec![home("h1")], vec![instance("i1", "h1")]);
        {
            let mut running = state.running.lock().await;
            running.insert("i1".to_string(), running_entry("web", Some("http://127.0.0.1:3089")));
            if let Some(entry) = running.get_mut("i1") {
                entry.adopted = Some(Adopted {
                    pid: 42,
                    port: 3089,
                    host: "127.0.0.1".to_string(),
                });
            }
        }
        let hit = running_for_profile(&state, "h1", "web", None).await.unwrap().unwrap();
        assert!(hit.adopted);
        assert_eq!(hit.url.as_deref(), Some("http://127.0.0.1:3089"));
    }
}
