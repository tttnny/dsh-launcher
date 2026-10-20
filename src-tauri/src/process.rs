use crate::config::{DshInstance, DshVersion, Host, HostStatus, InstanceState, InstanceStatus};
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
    /// The address the instance printed on **its own** machine, token included,
    /// once its log has shown it. It is a fact about that machine: the forward's
    /// target, and what the card shows as 「远端位置」. What this Mac reaches the
    /// instance at is the running entry's own `url` (ADR-0008).
    pub remote_url: Option<String>,
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

impl RemoteRun {
    /// Where one instance of `host` runs, as the launcher holds it: the machine,
    /// the pid that leads its group, when that pid started, and that machine's
    /// own data directory and log for the instance.
    fn of(
        host: &Host,
        run_host: crate::ssh::RemoteHost,
        data_dir: PathBuf,
        instance_id: &str,
        pid: i32,
        started_at: Option<u64>,
    ) -> Self {
        let log_path = data_dir
            .join("logs")
            .join(format!("{instance_id}.log"))
            .to_string_lossy()
            .to_string();
        Self {
            host_id: host.id.clone(),
            host: run_host,
            remote_url: None,
            pid,
            started_at,
            data_dir,
            log_path,
        }
    }
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
    args: String,
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
    let command = process_field(pid, "comm").await.unwrap_or_default();
    let args = process_field(pid, "args").await.unwrap_or_default();
    Some(PortOwner { pid, command, args })
}

/// One `ps -o <field>=` value of `pid`, trimmed. Best-effort.
async fn process_field(pid: i32, field: &str) -> Option<String> {
    let mut cmd = Command::new("ps");
    hide_console(&mut cmd);
    let out = cmd
        .args(["-p", &pid.to_string(), "-o", &format!("{field}=")])
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
/// binary. We match loosely so a packaged executable is recognized too — and
/// against the **command line** as well as the name, because a DSH shipped as a
/// single-file executable (a Python-packaged build is one) runs under a name of
/// its own (`MainThread`) and says `…/dsh/lib/bin.js` only in its argv.
fn looks_like_dsh(command: &str, args: &str) -> bool {
    let looks = |text: &str| {
        let c = text.to_ascii_lowercase();
        c.contains("node") || c.contains("dsh")
    };
    looks(command) || looks(args)
}

/// Verify the adopted pid still exists (cheap `kill -0` poll).
fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill(2) with signal 0 performs the permission+existence check
    // without delivering anything.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
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
                adopted: Some(Adopted {
                    pid,
                    port,
                    host: host.to_string(),
                }),
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
            remote_url: None,
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
                    remote_url: None,
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
            .filter(|inst| {
                inst.host_id.trim().is_empty() || inst.host_id == crate::config::LOCAL_HOST_ID
            })
            .filter_map(|inst| {
                let remembered = recall_url(&data_dir, &inst.id);
                let port = inst
                    .port
                    .or_else(|| remembered.as_deref().and_then(port_of_url))?;
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
        if !looks_like_dsh(&owner.command, &owner.args) {
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

fn write_remembered(data_dir: &std::path::Path, map: &serde_json::Map<String, serde_json::Value>) {
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
pub(crate) fn remember_url(
    data_dir: &std::path::Path,
    instance_id: &str,
    url: &str,
    profile: &str,
) {
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

/// Why 「打开」 has nothing to open, when that is the whole answer: the instance
/// is not in the running table at all.
const NOT_RUNNING: &str = "实例未在运行或尚未就绪";

/// Why 「打开」 has nothing to open, when the instance **is** running on that
/// machine and the launcher does not know its address.
///
/// These are two different states and they lead to two different actions, so
/// they are two different sentences: the first is answered by starting the
/// instance, the second (DSH replaced itself and the address it printed went to
/// a file the launcher was not following) by retrying or restarting it.
const ADDRESS_UNKNOWN: &str = "实例在那台机器上运行，但启动器不知道它的地址（DSH 自己重启后启动 token 没能找回来）。点「重试」再找一次；还找不到就从启动器重启这个实例。";

/// How much of a process's own output the launcher reads when it has to find the
/// address such an instance printed. Bounded on purpose: the line being looked
/// for is the newest one, and a long-lived instance's log can be gigabytes.
const ADDRESS_SEARCH_BYTES: usize = 64 * 1024;

/// How many of those addresses may be spent asking that machine before the
/// answer is 「不知道」. One restart prints a handful; a long-lived instance that
/// restarted a dozen times does not deserve a dozen ssh round trips.
const ADDRESS_CANDIDATES: usize = 8;

/// Every `dsh web:` address in one stretch of output, oldest first.
fn urls_in(text: &str) -> Vec<String> {
    text.lines().filter_map(url_in).collect()
}

/// The address one running remote instance printed, found by asking **that
/// machine** where the process's output goes and reading the newest address its
/// own loopback still accepts.
///
/// This is the state DSH's in-place restart leaves behind: the process keeps its
/// pid, so the launcher keeps believing it is the instance it started, while the
/// replacement writes its output — the `dsh web: <url>` line with the launch
/// token — to a file of its own making. The log the launcher has been following
/// then says nothing more, and the address is nowhere in the running record
/// either, because the exit path dropped it when the old pid went.
///
/// The token is spent **there** (`mint_headers`): the address belongs to that
/// machine's loopback, and a candidate carrying a token nothing accepts is not
/// this instance (ADR-0006). That is why nothing is returned unless one was
/// accepted — recovering an address is exactly the situation where a plausible
/// guess would be worse than saying 「不知道」.
async fn find_remote_address(state: &AppState, run: &RemoteRun) -> Option<String> {
    let transport = &*state.host_transport;
    // Each read hands back its own stretch oldest-first. Where the process
    // writes now is the most current place there is, so its addresses are looked
    // at before the log the launcher remembered — and within each one, newest
    // first, because an instance that replaced itself printed its address last.
    let mut sources: Vec<Vec<String>> = Vec::new();
    match crate::remote::stdout_tail(transport, &run.host, run.pid, ADDRESS_SEARCH_BYTES).await {
        Ok(tail) => sources.push(urls_in(&tail.text)),
        Err(e) => crate::log_warn!(
            "实例 pid {}（{}）的输出位置读不到：{e}",
            run.pid,
            run.host.target
        ),
    }
    match crate::remote::log_tail(transport, &run.host, &run.log_path, ADDRESS_SEARCH_BYTES).await {
        Ok(tail) if tail.exists => sources.push(urls_in(&tail.text)),
        Ok(_) => {}
        Err(e) => crate::log_warn!("实例 pid {} 的远端日志读不到：{e}", run.pid),
    }
    // Only addresses that carry a token are worth an ssh round trip, and the
    // same address seen in both places is asked about once.
    let candidates = sources.into_iter().flat_map(|urls| urls.into_iter().rev());
    let mut tried: Vec<String> = Vec::new();
    for url in candidates {
        if token_of(&url).is_none() || tried.contains(&url) {
            continue;
        }
        if tried.len() >= ADDRESS_CANDIDATES {
            break;
        }
        tried.push(url.clone());
        match crate::remote::mint_headers(transport, &run.host, &url).await {
            Ok(mint) => {
                let accepted = mint
                    .cookie
                    .as_deref()
                    .map(|pair| crate::native::cookie_pair_matches(&url, pair))
                    .unwrap_or(false);
                if accepted {
                    return Some(url);
                }
                crate::log_debug!(
                    "实例 pid {} 曾经打印的地址 {url} 没有被那台机器接受（HTTP {}）",
                    run.pid,
                    mint.status
                );
            }
            Err(e) => crate::log_warn!("在那台机器上确认 {url} 失败：{e}"),
        }
    }
    None
}

/// Re-establishes the address of a remote instance the launcher is holding
/// without one, and says which address that is on that machine — `None` when
/// nothing could be found and therefore nothing was changed.
///
/// What it records is the same thing the ready path records: the address that
/// machine printed, written into **that machine's** own running record so a
/// later session does not have to search again (ADR-0007 §3), and the forwarded
/// address in the running table, which is what 「打开」 hands over (ADR-0008).
/// A forward that will not come up costs the address, not the instance — the
/// caller retries it with the address it now has.
async fn repair_remote_address(
    state: &AppState,
    instance_id: &str,
    profile: &str,
    run: &RemoteRun,
) -> Option<String> {
    let remote_url = find_remote_address(state, run).await?;
    crate::log_info!(
        "实例 {instance_id} 在「{}」上运行的 pid {} 找回了地址：{remote_url}",
        run.host.target,
        run.pid
    );
    let record = RemoteRunRecord {
        url: Some(remote_url.clone()),
        token: token_of(&remote_url),
        profile: profile.to_string(),
        pid: run.pid,
        started_at: run.started_at,
    };
    if let Err(e) = remember_remote_run(state, instance_id, &record, run).await {
        // The address works for this session either way; losing the record only
        // costs the next session another search.
        crate::log_warn!("实例 {instance_id} 的远端运行记录没写上（{e}）");
    }
    let local_url = local_address_for(state, instance_id, &run.host, &remote_url).await;
    {
        let mut running = state.running.lock().await;
        if let Some(entry) = running.get_mut(instance_id) {
            // A start may have replaced the entry while the search ran: only
            // the run this search was about gets the address.
            if let Some(current) = entry.remote.as_mut() {
                if current.pid == run.pid {
                    current.remote_url = Some(remote_url.clone());
                    entry.url = local_url;
                }
            }
        }
    }
    Some(remote_url)
}

/// Emits the status of an instance whose address was just re-established: the
/// row states two addresses and only one of them was missing, so this is what
/// turns 「运行中」 into 「运行中，可打开」 without waiting for the next page load.
async fn emit_address_found(app: &AppHandle, state: &AppState, instance_id: &str, profile: &str) {
    let (url, remote_url) = {
        let running = state.running.lock().await;
        match running.get(instance_id) {
            Some(entry) => (
                entry.url.clone(),
                entry.remote.as_ref().and_then(|r| r.remote_url.clone()),
            ),
            None => (None, None),
        }
    };
    emit_status(
        app,
        &InstanceStatus {
            id: instance_id.to_string(),
            state: InstanceState::Running,
            url,
            remote_url,
            profile: Some(profile.to_string()),
            exit_code: None,
        },
    );
}

/// Puts one remote instance's address on this Mac, for an instance whose own
/// machine has just been found running it (ADR-0008).
///
/// `remote_url` is the address DSH printed on **its** machine — the forward's
/// target, and what the card shows as 「远端位置」. What comes back is the address
/// that works here, or `None` when the forward will not come up: that costs the
/// address, not the instance, which is running on that machine either way and
/// whose next 打开 builds the forward again and says why if it still cannot.
pub(crate) async fn local_address_for(
    state: &AppState,
    instance_id: &str,
    host: &crate::ssh::RemoteHost,
    remote_url: &str,
) -> Option<String> {
    match crate::tunnel::reachable_url(state, host, remote_url).await {
        Ok(local) => Some(local),
        Err(e) => {
            crate::log_warn!("实例 {instance_id} 的端口转发没建起来：{e}");
            None
        }
    }
}

/// The address **this Mac** can open a running instance at, bringing the
/// launcher's port forward up if the connection that carried it went away
/// (ADR-0008).
///
/// 本机's instances answer with their own URL, unchanged. A remote one answers
/// with the address its own machine printed, translated to loopback with the
/// path and the launch token carried over — which is the address the system
/// browser is handed, and the one the plugins page talks to. It is deliberately
/// not taken from the entry as it stands: a forward lives only as long as the
/// ssh connection under it, so the entry's copy can be an address that died
/// while the Mac slept, and this is where that is noticed and repaired.
///
/// A remote entry with no address at all — DSH replaced itself and its new
/// output file was never followed — is searched for on that machine first
/// ([`repair_remote_address`]), and only then answered 「不知道」.
pub(crate) async fn reachable_url_for(
    state: &AppState,
    instance_id: &str,
) -> Result<String, String> {
    let (url, remote, profile) = {
        let entry = state.running.lock().await;
        let Some(entry) = entry.get(instance_id) else {
            return Err(NOT_RUNNING.to_string());
        };
        (
            entry.url.clone(),
            entry.remote.clone(),
            entry.profile.clone(),
        )
    };
    let Some(run) = remote else {
        return url.ok_or_else(|| NOT_RUNNING.to_string());
    };
    let remote_url = match run.remote_url.clone() {
        Some(remote_url) => remote_url,
        None => repair_remote_address(state, instance_id, &profile, &run)
            .await
            .ok_or_else(|| ADDRESS_UNKNOWN.to_string())?,
    };
    crate::tunnel::reachable_url(state, &run.host, &remote_url).await
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
    host: &crate::ssh::RemoteHost,
    data_dir: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let path = remote_record_path(data_dir);
    let bytes = crate::remote::read_file_optional(&*state.host_transport, host, &path)
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
    host: &crate::ssh::RemoteHost,
    data_dir: &Path,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let path = remote_record_path(data_dir);
    let raw = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
        .map_err(|e| format!("序列化运行记录失败：{e}"))?;
    state
        .host_transport
        .write_file(host, &path, raw.into_bytes())
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
    let mut map = read_remote_records(state, &run.host, &run.data_dir).await?;
    let value = serde_json::to_value(record).map_err(|e| format!("序列化运行记录失败：{e}"))?;
    map.insert(instance_id.to_string(), value);
    write_remote_records(state, &run.host, &run.data_dir, &map).await
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
    let mut map = read_remote_records(state, &run.host, &run.data_dir).await?;
    if map.remove(instance_id).is_none() {
        return Ok(());
    }
    write_remote_records(state, &run.host, &run.data_dir, &map).await
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
        .unwrap_or_else(|| {
            home_path
                .join("profiles")
                .join(profile)
                .join("package.json")
        })
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

/// What one machine says about a profile's nature: whether a start has to treat
/// it as a DSH web application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileNature {
    /// That machine has the profile's own `package.json` and it declares the web
    /// bundle.
    Web,
    /// It has the document, and it does not: no web flags for this profile.
    NotWeb,
    /// There is no document yet — the profile is what this start materializes.
    /// The launcher has an answer for this case (the port policy), and DSH's own
    /// default port is not it.
    Unmaterialized,
}

/// Re-asserts the launcher's per-instance port in the profile's managed lan-bind
/// block, **on the machine that owns the profile**: that block outranks the CLI
/// `--port` and the plugin only re-asserts it at boot, so without this nudge the
/// first start after a port change comes up on the old port (see
/// [`crate::profile::assert_profile_lan_bind_port`] for the local twin).
///
/// A failure only costs the next start's port, so it is logged rather than
/// raised — the same weight the local one carries.
///
/// `document` is the patch file as the caller already read it, so a start that
/// has just fetched everything in one round trip does not fetch this again.
async fn assert_remote_lan_bind_port(
    state: &AppState,
    run_host: &crate::ssh::RemoteHost,
    home_path: &Path,
    profile: &str,
    port: u16,
    document: Option<Vec<u8>>,
) {
    let Some(patch) = crate::profile::lan_bind_patch_path(home_path, profile) else {
        return;
    };
    let Some(bytes) = document else {
        // Absent file: nothing to nudge.
        return;
    };
    let path = patch.to_string_lossy().to_string();
    let raw = String::from_utf8_lossy(&bytes).to_string();
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
///
/// The three documents arrive as the caller read them, for the same reason
/// [`assert_remote_lan_bind_port`] takes its patch: one start, one round trip.
/// The three profile documents one start reads from the machine that owns the
/// profile, already in hand: the caller fetched them in the same round trip as
/// everything else (`crate::remote::read_files`), so nothing here asks again.
struct ProfileDocuments {
    modules: Option<Vec<u8>>,
    workspace: Option<Vec<u8>>,
    npmrc: Option<Vec<u8>>,
}

async fn pin_remote_profile_store(
    state: &AppState,
    run_host: &crate::ssh::RemoteHost,
    home_path: &Path,
    profile: &str,
    data_dir: &Path,
    documents: ProfileDocuments,
) {
    let Ok(dir) = crate::profile::profile_dir(home_path, profile) else {
        return;
    };
    let text = |bytes: Option<Vec<u8>>| bytes.map(|b| String::from_utf8_lossy(&b).to_string());

    // The store the profile is already linked from, so a profile a user
    // installed against their own store keeps working.
    let linked = text(documents.modules)
        .as_deref()
        .and_then(crate::plugins::linked_store_document);
    let store = crate::plugins::store_target_from(
        linked.as_deref(),
        &crate::toolchain::store_dir(data_dir),
    );

    let workspace_path = dir.join("pnpm-workspace.yaml");
    let workspace_raw = text(documents.workspace);
    if let Some(out) = crate::plugins::store_pin_document(workspace_raw.as_deref(), &store) {
        let path = workspace_path.to_string_lossy().to_string();
        if let Err(e) = state
            .host_transport
            .write_file(run_host, &path, out.into_bytes())
            .await
        {
            crate::log_warn!("固定那台机器上 profile 的 pnpm store 失败（{path}）: {e}");
        }
    }

    let npmrc_path = dir.join(".npmrc");
    let npmrc_raw = text(documents.npmrc).unwrap_or_default();
    if let Some(out) = crate::plugins::npmrc_document(&npmrc_raw) {
        let path = npmrc_path.to_string_lossy().to_string();
        if let Err(e) = state
            .host_transport
            .write_file(run_host, &path, out.into_bytes())
            .await
        {
            crate::log_warn!("固定那台机器上 profile 的 peer 策略失败（{path}）: {e}");
        }
    }
}

/// That machine's log file for one instance:
/// `<该主机的数据目录>/logs/<实例 id>.log` — where a remote spawn appends its
/// output, and what 「日志」 copies the tail of. One derivation, so the file the
/// launcher reads is the file the spawn wrote. Not an install step's log, which
/// `tasks::remote_step_log_path` names.
pub(crate) fn remote_instance_log_path(data_dir: &Path, instance_id: &str) -> String {
    data_dir
        .join("logs")
        .join(format!("{instance_id}.log"))
        .to_string_lossy()
        .to_string()
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

    // Everything this start has to know about that machine's files, asked in
    // **one** round trip: the version's `bin.js`, the profile's manifest, the
    // version's web-app bundle, and the three profile documents the writes below
    // may touch. One question per file would be honest and slow — on a machine
    // whose login shell takes a second to start, a second per file — and the
    // answers all feed one decision: what argv to spawn.
    let bin = crate::launch::version_bin(&version.dir);
    let home_path = cfg
        .homes
        .iter()
        .find(|h| h.id == inst.home_id)
        .map(|h| h.path.clone());
    let profile_pkg = home_path
        .as_deref()
        .map(|home| profile_package_json(home, profile));
    let startup = crate::launch::web_app_startup_script(&version.dir);
    let patch = home_path
        .as_deref()
        .and_then(|home| crate::profile::lan_bind_patch_path(home, profile));
    let profile_dir = home_path
        .as_deref()
        .and_then(|home| crate::profile::profile_dir(home, profile).ok());

    // One path per question, built up front: a slot that was never asked for
    // stays `None`, and its absence means the same thing as 「没有这份文档」.
    let modules_path = profile_dir
        .as_ref()
        .map(|d| d.join("node_modules/.modules.yaml"));
    let workspace_path = profile_dir.as_ref().map(|d| d.join("pnpm-workspace.yaml"));
    let npmrc_path = profile_dir.as_ref().map(|d| d.join(".npmrc"));

    let mut paths: Vec<String> = vec![bin.to_string_lossy().to_string()];
    let optional: [Option<&Path>; 6] = [
        profile_pkg.as_deref(),
        Some(startup.as_path()),
        patch.as_deref(),
        modules_path.as_deref(),
        workspace_path.as_deref(),
        npmrc_path.as_deref(),
    ];
    for path in optional.iter().flatten() {
        paths.push(path.to_string_lossy().to_string());
    }
    let mut answers = crate::remote::read_files(&*state.host_transport, &run_host, &paths)
        .await
        .map_err(|e| format!("无法读取「{}」上的文件：{e}", host.display_name))?
        .into_iter();
    let bin_bytes = answers.next().flatten();
    // Slots line up with `optional`: a file that was not asked for reads as
    // 「没有这份文档」, which is exactly what its absence means below.
    let mut next = |asked: bool| -> Option<Vec<u8>> {
        if asked {
            answers.next().flatten()
        } else {
            None
        }
    };

    // The version's own `bin.js`: `version_bin_ready` is a local `exists()`,
    // which at a remote path answers about this Mac.
    if bin_bytes.as_deref().map(<[u8]>::is_empty).unwrap_or(true) {
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
    //
    // A profile with **no** document there yet is the third case, and it is the
    // one that must not be read as 「not a web app」: this start is about to
    // materialize it, and a start with no port falls back to DSH's own default
    // (3080) — a port something else may already hold on that machine, which
    // turns a start into an unexplained death instead of the launcher's own port
    // policy. So an unmaterialized profile gets the intended port like any web
    // profile does (`0` = that machine's kernel picks a free one; a pinned port
    // was preflighted, and its occupant was named before anything was started).
    let nature = match (home_path.as_deref(), next(profile_pkg.is_some())) {
        (Some(_), Some(bytes)) => {
            if web_profile_document(&String::from_utf8_lossy(&bytes)) {
                ProfileNature::Web
            } else {
                ProfileNature::NotWeb
            }
        }
        (Some(_), None) if profile_pkg.is_some() => ProfileNature::Unmaterialized,
        // No HOME record to ask about: the profile's own name is all there is.
        _ => {
            if profile == "web" {
                ProfileNature::Web
            } else {
                ProfileNature::NotWeb
            }
        }
    };
    let startup_doc = next(true);
    let patch_doc = next(patch.is_some());
    let has_profile_dir = profile_dir.is_some();
    let modules_doc = next(has_profile_dir);
    let workspace_doc = next(has_profile_dir);
    let npmrc_doc = next(has_profile_dir);

    let is_web = nature != ProfileNature::NotWeb;
    let web_port = is_web.then(|| inst.port.unwrap_or(0));

    if let (true, Some(home)) = (is_web, home_path.as_deref()) {
        assert_remote_lan_bind_port(
            state,
            &run_host,
            home,
            profile,
            inst.port.unwrap_or(0),
            patch_doc,
        )
        .await;
    }
    if let Some(home) = home_path.as_deref() {
        pin_remote_profile_store(
            state,
            &run_host,
            home,
            profile,
            &data_dir,
            ProfileDocuments {
                modules: modules_doc,
                workspace: workspace_doc,
                npmrc: npmrc_doc,
            },
        )
        .await;
    }

    // The interpreter is that machine's resolved binding (ADR-0005 追加), and
    // the flags are the launcher's policy — `--host` deliberately never among
    // them, `--no-open` only when that machine's bundle understands it.
    let node = crate::runtime::node_for_spawn_checked_for(state, Some(&host.id)).await?;
    let no_open = web_port.is_some()
        && startup_doc
            .as_deref()
            .map(|bytes| crate::launch::supports_no_open(&String::from_utf8_lossy(bytes)))
            .unwrap_or(false);
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

    let log_path = crate::process::remote_instance_log_path(&data_dir, &inst.id);

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
    // A pid whose start time was never written down cannot be stopped from here
    // (the stop refuses rather than signal a pid it cannot identify), so the one
    // read that may fail transiently gets a second try before that is the
    // answer. Failing twice is recorded honestly as `None`, never as a guess.
    let started_at = match started_at {
        Some(_) => started_at,
        None => {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let retry = crate::remote::process_start_time(&*state.host_transport, &run_host, pid)
                .await
                .ok()
                .flatten();
            if retry.is_none() {
                crate::log_warn!(
                    "读不到远端 pid {pid} 的启动时间：停止时会先要求核对，而不是直接发信号"
                );
            }
            retry
        }
    };

    let run = RemoteRun {
        host_id: host.id.clone(),
        host: run_host,
        remote_url: None,
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

/// How many bytes of a priming boot's output the launcher keeps for the failure
/// message, and for how long it waits for the web URL.
const REMOTE_PRIME_LOG_LINES: usize = 200;
const REMOTE_PRIME_WAIT: std::time::Duration = std::time::Duration::from_secs(90);

/// Gives a brand-new DSH_HOME on another machine the same start it gets here.
///
/// A DSH_HOME DSH has never run in has no `profiles/` at all, and DSH
/// materializes `profiles/web` on its **first** run. 本机's dedicated HOME gets
/// that from `tasks::ensure_web_profile_template` while the version installs;
/// without the twin of that step, a fresh remote HOME leaves the page with an
/// empty Profile list and 「启动」 with nothing to launch.
///
/// So that machine runs the same throwaway boot the local path runs — `node
/// bin.js --profile web --host 127.0.0.1 --port 0`, DSH_HOME pointed at the HOME
/// — and is stopped again once it has printed its URL; `profiles/web` is then
/// copied to `profiles/__temp__`, the template later profiles are derived from,
/// with the port the boot happened to bind scrubbed out of the copy.
///
/// A HOME that already has a profile is left completely alone: this is for the
/// HOME nobody has run DSH in, not a repair for one that has.
pub(crate) async fn ensure_remote_web_profile_template(
    state: &AppState,
    host: &Host,
    version: &DshVersion,
    home_path: &Path,
) -> Result<(), String> {
    let transport = &*state.host_transport;
    let run_host = crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;
    let data_dir = host.data_dir(&state.data_dir)?;
    let profiles = crate::profile::profiles_dir(home_path);

    // A HOME that already has a profile is the user's own business: the first
    // boot is what makes a Profile exist, and this is only for the HOME where
    // none does yet.
    let listed = crate::remote::list_profiles(transport, &run_host, &profiles.to_string_lossy())
        .await
        .map_err(|e| format!("无法读取「{}」上的 Profile 列表：{e}", host.display_name))?;
    if !listed.is_empty() {
        return Ok(());
    }

    let bin = crate::launch::version_bin(&version.dir);
    let ready = crate::remote::file_ready(transport, &run_host, &bin.to_string_lossy())
        .await
        .map_err(|e| format!("无法确认那台机器上的 {}：{e}", bin.display()))?;
    if !ready {
        return Err(format!(
            "版本 {} 安装不完整（缺少 {}），请重新安装",
            version.version,
            bin.display()
        ));
    }

    let node = crate::runtime::node_for_spawn_checked_for(state, Some(&host.id)).await?;
    // `--port 0`: the kernel on that machine picks a free port for a boot nobody
    // keeps, so it can never take one an instance wants.
    let mut argv = vec![
        "/usr/bin/env".to_string(),
        format!("DSH_HOME={}", home_path.to_string_lossy()),
    ];
    argv.extend(crate::launch::template_boot_argv(&node, &version.dir, 0));

    let log_path = remote_prime_log_path(&data_dir, home_path);
    // Read the log's size **before** the process exists: a previous attempt's URL
    // line is not this one's, and a HOME primed twice must not mistake one for
    // the other.
    let offset = crate::remote::log_size(transport, &run_host, &log_path)
        .await
        .map_err(|e| format!("无法读取那台机器上的日志（{log_path}）：{e}"))?;
    let pid = state
        .host_transport
        .spawn_detached(&run_host, &argv, &log_path)
        .await
        .map_err(|e| format!("在「{}」上启动失败：{e}", host.display_name))?;
    let started_at = crate::remote::process_start_time(transport, &run_host, pid)
        .await
        .ok()
        .flatten();
    let run = RemoteRun {
        host_id: host.id.clone(),
        host: run_host,
        remote_url: None,
        pid,
        started_at,
        data_dir: data_dir.clone(),
        log_path: log_path.clone(),
    };

    let mut offset = offset;
    let mut pending = String::new();
    let mut tail: Vec<String> = Vec::new();
    let mut ready = false;
    let deadline = std::time::Instant::now() + REMOTE_PRIME_WAIT;
    loop {
        let chunk = crate::remote::tail(
            transport,
            &run.host,
            &log_path,
            offset,
            pid,
            REMOTE_TAIL_WAIT_SECS,
        )
        .await
        .map_err(|e| format!("读取那台机器上的日志失败（{log_path}）：{e}"))?;
        offset = chunk.offset;
        pending.push_str(&chunk.text);
        while let Some(cut) = pending.find('\n') {
            let line = pending[..cut].trim_end_matches('\r').to_string();
            pending.drain(..=cut);
            if line.trim().is_empty() {
                continue;
            }
            if url_in(&line).is_some() {
                ready = true;
            }
            if tail.len() >= REMOTE_PRIME_LOG_LINES {
                tail.remove(0);
            }
            tail.push(line);
        }
        if ready || !chunk.alive || std::time::Instant::now() >= deadline {
            break;
        }
    }

    // Stop it before anything else: the boot was only there to make `profiles/`
    // exist, and a DSH left running would hold a port on that machine for good.
    if let Some(said) = prime_stop_failure(host, pid, stop_remote_run(state, &run).await) {
        return Err(said);
    }

    if !ready {
        return Err(format!(
            "在「{}」上生成 web profile 超时或失败：{}",
            host.display_name,
            prime_log_summary(&tail)
        ));
    }

    // The template later profiles are derived from, made the way 本机 makes it.
    let web_dir = profiles.join("web");
    let temp_dir = profiles.join("__temp__");
    crate::remote::copy_profile(
        transport,
        &run.host,
        &web_dir.to_string_lossy(),
        &temp_dir.to_string_lossy(),
    )
    .await
    .map_err(|e| format!("复制 __temp__ profile 失败：{e}"))?;
    scrub_remote_profile_port_pin(state, &run.host, &temp_dir).await;
    crate::log_info!(
        "「{}」上 {} 的 web profile 模板已建好（pid {pid}）",
        host.display_name,
        home_path.display()
    );
    Ok(())
}

/// That machine's log for one HOME's priming boot. A file per HOME, because two
/// HOMEs primed at once must not read each other's URL line — and not the
/// instance's own log, which 「日志」 shows the user.
fn remote_prime_log_path(data_dir: &Path, home_path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(home_path.to_string_lossy().as_bytes());
    let short: String = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    data_dir
        .join("logs")
        .join(format!("profile-{short}.log"))
        .to_string_lossy()
        .to_string()
}

/// Why a priming boot that is still there could not be stopped, or `None` when
/// it did stop. A process the launcher cannot identify is never signalled
/// (ADR-0007 §3), and one that will not go stays where it is — either way the
/// user is told, because a DSH left running on their server is theirs to clean
/// up, not the launcher's to forget.
fn prime_stop_failure(host: &Host, pid: i32, outcome: RemoteStop) -> Option<String> {
    match outcome {
        RemoteStop::Stopped => None,
        RemoteStop::Unreachable(reason) => Some(format!(
            "「{}」上生成 web profile 的进程（pid {pid}）没能停下：{reason}",
            host.display_name
        )),
        RemoteStop::NotSignalled(reason)
        | RemoteStop::Reused(reason)
        | RemoteStop::StillRunning(reason) => Some(format!(
            "「{}」上生成 web profile 的进程（pid {pid}）还在：{reason}",
            host.display_name
        )),
    }
}

/// Why a priming boot never printed a URL: the last thing it said, errors
/// first. The same shape the local template boot reports with, so both machines
/// fail the same way.
fn prime_log_summary(tail: &[String]) -> String {
    let errors: Vec<&String> = tail
        .iter()
        .filter(|line| line.to_lowercase().contains("error"))
        .collect();
    let source: &[&String] = if errors.is_empty() {
        &tail.iter().collect::<Vec<_>>()
    } else {
        &errors
    };
    let said = source
        .iter()
        .rev()
        .take(2)
        .rev()
        .map(|line| line.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    if said.is_empty() {
        "DSH 没有打印出任何输出".to_string()
    } else {
        said
    }
}

/// Scrubs the port pin out of a copied profile's patch document, on the machine
/// that holds the copy ([`crate::profile::scrub_profile_port_pin`] is the local
/// twin). Best effort: a template with a pin costs the next start a port, not a
/// functioning HOME.
async fn scrub_remote_profile_port_pin(
    state: &AppState,
    run_host: &crate::ssh::RemoteHost,
    profile_dir: &Path,
) {
    let patch = profile_dir.join("cordis.patch.yml");
    let path = patch.to_string_lossy().to_string();
    let raw = match crate::remote::read_file_optional(&*state.host_transport, run_host, &path).await
    {
        Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
        Ok(None) | Err(_) => return,
    };
    let scrubbed = crate::profile::scrub_port_pin(&raw);
    if scrubbed == raw {
        return;
    }
    if let Err(e) = state
        .host_transport
        .write_file(run_host, &path, scrubbed.into_bytes())
        .await
    {
        crate::log_warn!("清掉那台机器上 profile 模板的端口钉死失败（{path}）: {e}");
    }
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
            remote_url: None,
            profile: Some(profile.to_string()),
            exit_code: None,
        },
    );

    // What that machine already has, before anything is started there. This is
    // the remote half of the pinned-port preflight in `start_instance_process`:
    // a pinned port held by something that is not DSH is named instead of dying
    // with a remote EADDRINUSE, and one held by DSH is the instance itself —
    // adopted, not started a second time.
    //
    // It runs for random ports too, which is where it earns its place: the
    // record that machine keeps is the only way to notice that an instance the
    // launcher failed to adopt is *still running*, and starting a second DSH on
    // the server is the one outcome ADR-0007 exists to prevent.
    match check_remote_instance(state, inst, host).await {
        RemoteVerdict::Running {
            pid,
            started_at,
            url,
            profile: running_profile,
        } => {
            state.set_host_status(&host.id, HostStatus::reachable());
            let run_host =
                crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;
            let data_dir = host.data_dir(&state.data_dir)?;
            adopt_remote_process(
                app,
                state,
                &inst.id,
                &running_profile,
                RemoteRun::of(host, run_host, data_dir, &inst.id, pid, started_at),
                url,
            )
            .await;
            return Ok(());
        }
        RemoteVerdict::Occupied { pid, command } => {
            return Err(format!(
                "端口 {} 在「{}」上已被其他进程占用（pid {}，{}），请先释放该端口或修改实例端口",
                inst.port.unwrap_or(0),
                host.display_name,
                pid.map(|p| p.to_string())
                    .unwrap_or_else(|| "未知".to_string()),
                if command.is_empty() {
                    "未知进程"
                } else {
                    &command
                }
            ));
        }
        // Nothing is running there, or the machine could not be asked: the
        // spawn below is what has something to say about the second case, and
        // it says it better than this preflight could.
        RemoteVerdict::Stopped | RemoteVerdict::Unreachable(_) => {}
    }

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
                    remote_url: None,
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

            if let Some(remote_url) = read.url.filter(|_| !announced) {
                announced = true;
                crate::log_info!("实例 {id} 已就绪：{remote_url}");
                // The address DSH printed is about **that** machine. What this
                // Mac can open is the same address brought over on a loopback
                // forward (ADR-0008), and it is brought up now rather than at
                // the first click: the row has both addresses to state, and the
                // plugins page talks to the instance through the forward too.
                let local_url = local_address_for(&state, &id, &run.host, &remote_url).await;
                {
                    let mut running = state.running.lock().await;
                    if let Some(entry) = running.get_mut(&id) {
                        entry.url = local_url.clone();
                        if let Some(run) = entry.remote.as_mut() {
                            run.remote_url = Some(remote_url.clone());
                        }
                    }
                }
                emit_status(
                    &app,
                    &InstanceStatus {
                        id: id.clone(),
                        state: InstanceState::Running,
                        url: local_url,
                        remote_url: Some(remote_url),
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
                        remote_url: None,
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

// ---------------------------------------------------------------------------
// Stopping, checking and re-adopting an instance that runs on another machine
//
// Everything here is the remote half of what `stop_instance_process` /
// `adopt_surviving_instances` do locally, and the rules are the same ones — they
// are simply asked of the machine that owns the process. Two of them matter more
// than the rest:
//
// * A pid is not an identity (ADR-0007 §3): the process start time recorded when
//   it was started is checked against that machine's `/proc/<pid>/stat` before
//   anything is signalled, because signalling a reused pid is the one mistake
//   here that cannot be undone.
// * 「已停止」 is only ever said about a process the launcher has seen go. A
//   machine that cannot answer leaves the instance 「主机不可达」
//   ([`InstanceState::Unreachable`]), never 「已停止」: that is the value that
//   makes a user start a second DSH on the server (ADR-0007 后果).
// ---------------------------------------------------------------------------

/// How long a remote stop waits **on that machine** for the process to go. The
/// wait happens there ([`crate::remote::wait_gone`]), so this is one call and
/// not one per second.
const REMOTE_STOP_WAIT_SECS: u64 = 20;

/// How often a live remote instance is asked whether it is still there, and how
/// much longer the interval gets while that machine cannot answer.
const REMOTE_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
const REMOTE_WATCH_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);

/// What one look at a remote instance found: the conclusion the page needs, not
/// the steps that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteVerdict {
    /// A DSH-like process holds this instance's port on that machine.
    Running {
        pid: i32,
        started_at: Option<u64>,
        /// The URL that machine's record kept — the only place the launch token
        /// survives a launcher restart.
        url: Option<String>,
        profile: String,
    },
    /// Nothing of this instance is running there.
    Stopped,
    /// The instance's **pinned** port is held by something that is not DSH. The
    /// instance is not running, and a start must name who is in the way instead
    /// of dying with EADDRINUSE.
    Occupied { pid: Option<i32>, command: String },
    /// The machine could not answer, so nothing can be said about the instance.
    Unreachable(String),
}

/// What stopping a remote instance did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteStop {
    /// Confirmed gone — by the signal, or it was already gone.
    Stopped,
    /// Nothing was signalled, and why: a pid that is not the process the
    /// launcher started, or a start time that cannot be checked at all.
    NotSignalled(String),
    /// The pid now belongs to a **different** process — the kernel reused the
    /// number, so the one the launcher started has exited. Nothing was
    /// signalled, and nothing about this instance is left running: the caller
    /// must forget the entry and report it exited.
    Reused(String),
    /// The signal went and the process did not go.
    StillRunning(String),
    /// The machine did not answer.
    Unreachable(String),
}

/// The remote half of a stop, from the identity check to the confirmation. The
/// caller owns the projections (the status event, the tray, the record).
///
/// The three steps are **one** call ([`crate::remote::stop_run`]): the identity
/// check is about the process the signal is aimed at, so it has to happen in the
/// same shell as the signal, and a machine whose login shell is slow pays for it
/// once rather than three times.
pub(crate) async fn stop_remote_run(state: &AppState, run: &RemoteRun) -> RemoteStop {
    let target = run.host.target.clone();
    let outcome = crate::remote::stop_run(
        &*state.host_transport,
        &run.host,
        run.pid,
        run.started_at,
        REMOTE_STOP_WAIT_SECS,
    )
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            crate::log_warn!("停止远端实例失败（{target}，pid {}）：{e}", run.pid);
            return RemoteStop::Unreachable(format!(
                "在「{target}」上停止 pid {} 失败：{e}",
                run.pid
            ));
        }
    };
    match outcome {
        // Not there at all: nothing to stop, and nothing to report as a failure
        // either (the second click on 停止 is not an error).
        StopOutcome::Stopped | StopOutcome::Gone => RemoteStop::Stopped,
        StopOutcome::Reused { now, recorded } => RemoteStop::Reused(format!(
            "pid {} 在「{target}」上已被系统复用（现在的启动时间 {}，启动时记下的是 {}）：没有发信号",
            run.pid,
            now.map(|n| n.to_string()).unwrap_or_else(|| "读不到".to_string()),
            recorded
                .map(|n| n.to_string())
                .unwrap_or_else(|| "没记下".to_string())
        )),
        StopOutcome::Unreadable => RemoteStop::NotSignalled(format!(
            "「{target}」上读不到 pid {} 的启动时间（/proc/<pid>/stat 打不开），没法确认它还是启动时那个进程：没有发信号。请在那台机器上核对后自行停止：ssh {target} 'kill -TERM -- -{}'",
            run.pid, run.pid
        )),
        // The start time was never recorded (that machine could not be read at
        // spawn time). Saying so is the honest half of "check before you signal";
        // 重试 (which re-establishes the instance) is what fills the record in.
        StopOutcome::Unprepared => RemoteStop::NotSignalled(format!(
            "启动时没能记下 pid {} 的启动时间，现在无法确认它还是那个进程：没有发信号。刷新一次可以重新核对这个 pid；也可以在那台机器上停：ssh {target} 'kill -TERM -- -{}'",
            run.pid, run.pid
        )),
        StopOutcome::Refused => RemoteStop::NotSignalled(format!(
            "「{target}」上给 pid {} 发终止信号被拒绝（权限不足？），进程还在",
            run.pid
        )),
        StopOutcome::Lingering => RemoteStop::StillRunning(format!(
            "远端进程（{target}，pid {}）在 {REMOTE_STOP_WAIT_SECS} 秒内没有响应终止信号",
            run.pid
        )),
    }
}

/// One instance's entry out of that machine's own running record.
async fn recall_remote_run(
    state: &AppState,
    host: &crate::ssh::RemoteHost,
    data_dir: &Path,
    instance_id: &str,
) -> Result<Option<RemoteRunRecord>, String> {
    let map = read_remote_records(state, host, data_dir).await?;
    Ok(map
        .get(instance_id)
        .and_then(|value| serde_json::from_value::<RemoteRunRecord>(value.clone()).ok()))
}

/// The two things every call to another machine needs — how to reach that
/// machine, and where its data directory is — or the reason this Mac cannot
/// work them out. Both are facts about the *host*, not about one instance, so
/// both are reported the same way wherever they block a call: a host the
/// launcher cannot place is a host it cannot ask. The directory's own reason is
/// kept as [`Host::data_dir`] wrote it: it is what the row shows as the
/// explanation of 「主机不可达」, and it carries the way out of that state.
fn remote_parts(
    host: &Host,
    launcher_data_dir: &Path,
) -> Result<(crate::ssh::RemoteHost, PathBuf), String> {
    let run_host = crate::ssh::RemoteHost::of(host, launcher_data_dir)
        .map_err(|_| format!("「{}」是本机，它上面没有远端进程", host.display_name))?;
    let data_dir = host.data_dir(launcher_data_dir)?;
    Ok((run_host, data_dir))
}

/// What state one remote instance is in right now, as the machine that runs it
/// reports it.
///
/// The rule is 本机's, asked of that machine (ADR-0007 §2): the port the instance
/// was pinned to — or the port in the record it left behind — is where to look.
/// A **pinned** port is evidence by itself as soon as a DSH-like process is
/// listening on it; a **random** port has to prove itself with the token from
/// that machine's record, or the port may belong to something else entirely.
///
/// The token is spent **there** ([`crate::remote::mint_headers`]): a remote
/// instance listens on that machine's own loopback, so the same URL asked from
/// this Mac would reach a different service.
pub(crate) async fn check_remote_instance(
    state: &AppState,
    inst: &DshInstance,
    host: &Host,
) -> RemoteVerdict {
    let (run_host, data_dir) = match remote_parts(host, &state.data_dir) {
        Ok(parts) => parts,
        Err(reason) => return RemoteVerdict::Unreachable(reason),
    };
    let transport = &*state.host_transport;
    let record = match recall_remote_run(state, &run_host, &data_dir, &inst.id).await {
        Ok(record) => record,
        Err(e) => return RemoteVerdict::Unreachable(e),
    };
    let profile = record
        .as_ref()
        .map(|r| r.profile.clone())
        .or_else(|| inst.last_profile.clone())
        .or_else(|| inst.default_profile.clone())
        .unwrap_or_else(|| "web".to_string());
    let url = record.as_ref().and_then(|r| r.url.clone());
    // The port to look at: the one the user pinned, or the one that machine's
    // record saw the instance come up on.
    let Some(port) = inst.port.or_else(|| url.as_deref().and_then(port_of_url)) else {
        // No address to look at: a non-web profile has none, and an instance
        // that never printed a URL left nothing to find.
        return RemoteVerdict::Stopped;
    };
    let owner = match crate::remote::port_owner(transport, &run_host, port).await {
        Ok(owner) => owner,
        Err(e) => {
            return RemoteVerdict::Unreachable(format!(
                "在「{}」上查端口 {port} 的占用者失败：{e}",
                host.display_name
            ))
        }
    };
    let Some(owner) = owner else {
        return RemoteVerdict::Stopped;
    };
    // A listener this user cannot name (somebody else's process, or no
    // permission to read its name) is still an occupied port — and an occupied
    // *pinned* port is what the start has to report.
    let Some(pid) = owner.pid else {
        return if inst.port.is_some() {
            RemoteVerdict::Occupied {
                pid: None,
                command: owner.command,
            }
        } else {
            RemoteVerdict::Stopped
        };
    };
    if !looks_like_dsh(&owner.command, &owner.args) {
        return if inst.port.is_some() {
            RemoteVerdict::Occupied {
                pid: Some(pid),
                command: owner.command,
            }
        } else {
            RemoteVerdict::Stopped
        };
    }
    if inst.port.is_none() {
        // A random port says nothing on its own: the token that machine wrote
        // down is the only thing that turns a DSH-like listener into *this*
        // instance.
        let Some(url) = url.as_deref() else {
            return RemoteVerdict::Stopped;
        };
        let mint = match crate::remote::mint_headers(transport, &run_host, url).await {
            Ok(mint) => mint,
            Err(e) => {
                return RemoteVerdict::Unreachable(format!(
                    "在「{}」上确认实例 {url} 失败：{e}",
                    host.display_name
                ))
            }
        };
        let accepted = mint
            .cookie
            .as_deref()
            .map(|pair| crate::native::cookie_pair_matches(url, pair))
            .unwrap_or(false);
        if !accepted {
            crate::log_debug!(
                "实例 {} 在那台机器的端口 {port} 上有 DSH 样的进程，但记下的 token 没有被接受（HTTP {}）：不认领",
                inst.id,
                mint.status
            );
            return RemoteVerdict::Stopped;
        }
    }
    // The pid to record came off that machine's own listener, so this read is
    // also what repairs a record whose start time was never written.
    let started_at = crate::remote::process_start_time(transport, &run_host, pid)
        .await
        .ok()
        .flatten();
    RemoteVerdict::Running {
        pid,
        started_at,
        url,
        profile,
    }
}

/// Registers a process that is already running on a remote host as this
/// instance's running entry — the remote twin of [`adopt_external_process`],
/// and what makes a restarted launcher manageable again: the URL with its launch
/// token comes out of that machine's own running record (ADR-0007 §3).
///
/// That address is about **that** machine, so it is kept as the run's own
/// address —「远端位置」, and the forward's target — while the entry is given the
/// address this Mac can reach: the same one on a loopback forward (ADR-0008),
/// which is what 「打开」, the tray and the plugins page read.
async fn adopt_remote_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
    profile: &str,
    mut run: RemoteRun,
    remote_url: Option<String>,
) {
    let local_url = match remote_url.as_deref() {
        Some(remote) => {
            run.remote_url = Some(remote.to_string());
            local_address_for(state, instance_id, &run.host, remote).await
        }
        None => None,
    };
    {
        let mut running = state.running.lock().await;
        if running.contains_key(instance_id) {
            return; // A start registered first; it wins.
        }
        running.insert(
            instance_id.to_string(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: profile.to_string(),
                url: local_url.clone(),
                adopted: None,
                remote: Some(run.clone()),
            },
        );
    }
    match remote_url.as_deref() {
        Some(remote) => crate::log_info!(
            "实例 {instance_id} 在「{}」上仍在运行（pid {}），已认领，沿用记下的启动 token（{remote}）",
            run.host.target,
            run.pid
        ),
        None => crate::log_info!(
            "实例 {instance_id} 在「{}」上仍在运行（pid {}），已认领（没有可用的 token）",
            run.host.target,
            run.pid
        ),
    }
    emit_status(
        app,
        &InstanceStatus {
            id: instance_id.to_string(),
            state: InstanceState::Running,
            url: local_url,
            remote_url,
            profile: Some(profile.to_string()),
            exit_code: None,
        },
    );
    crate::tray::rebuild_tray_menu(app).await;
    watch_remote_process(app, instance_id, profile, run);
}

/// Watches an adopted remote instance: the machine is asked whether the process
/// is still there, and the answer is what ends the watch.
///
/// A machine that cannot answer is not an exit. It is recorded as such — the
/// host's status says 不可达, which is what makes the instance read 「主机不可达」
/// rather than 「已停止」 — and the watch keeps trying at a lower rate, so an
/// instance that comes back (a reboot of the server, a network that returns)
/// converges without the user doing anything.
fn watch_remote_process(app: &AppHandle, instance_id: &str, profile: &str, run: RemoteRun) {
    let app = app.clone();
    let id = instance_id.to_string();
    let profile = profile.to_string();
    tauri::async_runtime::spawn(async move {
        let mut interval = REMOTE_WATCH_INTERVAL;
        let mut reported_lost = false;
        loop {
            tokio::time::sleep(interval).await;
            let state = app.state::<AppState>();
            // Stop coordinating when the entry is gone (the user stopped it) or
            // was replaced (a start raced this adoption).
            let mine = state
                .running
                .lock()
                .await
                .get(&id)
                .and_then(|entry| entry.remote.as_ref())
                .map(|entry| entry.pid == run.pid)
                .unwrap_or(false);
            if !mine {
                return;
            }
            match crate::remote::alive(&*state.host_transport, &run.host, run.pid).await {
                Ok(true) => {
                    if reported_lost {
                        reported_lost = false;
                        state.set_host_status(&run.host_id, HostStatus::reachable());
                        // The URL comes back with it: it is the launch token's
                        // home, and dropping it would leave a running instance
                        // unmanageable in the page for no reason.
                        let url = state
                            .running
                            .lock()
                            .await
                            .get(&id)
                            .and_then(|entry| entry.url.clone());
                        emit_status(
                            &app,
                            &InstanceStatus {
                                id: id.clone(),
                                state: InstanceState::Running,
                                url,
                                remote_url: run.remote_url.clone(),
                                profile: Some(profile.clone()),
                                exit_code: None,
                            },
                        );
                    }
                    interval = REMOTE_WATCH_INTERVAL;
                }
                Ok(false) => {
                    crate::log_info!(
                        "实例 {id} 在「{}」上已退出（pid {}）",
                        run.host.target,
                        run.pid
                    );
                    unregister_running(
                        &app,
                        &state,
                        &id,
                        InstanceStatus {
                            id: id.clone(),
                            state: InstanceState::Exited,
                            url: None,
                            remote_url: None,
                            profile: Some(profile.clone()),
                            exit_code: None,
                        },
                    )
                    .await;
                    return;
                }
                Err(e) => {
                    crate::log_warn!("实例 {id} 的远端进程暂时问不到（{}）：{e}", run.host.target);
                    reported_lost = true;
                    state.set_host_status(
                        &run.host_id,
                        HostStatus::unreachable(format!("读取远端实例状态失败：{e}")),
                    );
                    emit_status(
                        &app,
                        &InstanceStatus {
                            id: id.clone(),
                            state: InstanceState::Unreachable,
                            url: None,
                            remote_url: run.remote_url.clone(),
                            profile: Some(profile.clone()),
                            exit_code: None,
                        },
                    );
                    interval = REMOTE_WATCH_BACKOFF;
                }
            }
        }
    });
}

/// Establishes — or re-establishes — one remote instance's state, and projects
/// it: an adopted process becomes a running entry, and a machine that answered
/// 「没有」 says so, so a card left over from an earlier session converges.
///
/// This is the one place the launch-time scan and the page's 重试 share, so the
/// two can never disagree about what that machine said.
pub(crate) async fn settle_remote_instance(
    app: &AppHandle,
    state: &State<'_, AppState>,
    inst: &DshInstance,
    host: &Host,
) -> InstanceState {
    // Already running as far as this session is concerned. Two things are worth
    // re-reading here, and both are 重试's answer: the process start time (a stop
    // refuses without it, and this is how a record that missed it gets repaired)
    // and the address (DSH can replace itself in place, leaving this session
    // holding a live pid whose launch token went with the process it replaced).
    let existing = {
        let running = state.running.lock().await;
        running
            .get(&inst.id)
            .map(|entry| (entry.remote.clone(), entry.profile.clone()))
    };
    if let Some((existing_run, profile)) = existing {
        if let Some(run) = existing_run {
            if run.started_at.is_none() {
                if let Ok(Some(at)) =
                    crate::remote::process_start_time(&*state.host_transport, &run.host, run.pid)
                        .await
                {
                    let mut running = state.running.lock().await;
                    if let Some(entry) = running.get_mut(&inst.id) {
                        if let Some(remote) = entry.remote.as_mut() {
                            remote.started_at = Some(at);
                        }
                    }
                }
            }
            if run.remote_url.is_none()
                && repair_remote_address(state, &inst.id, &profile, &run)
                    .await
                    .is_some()
            {
                emit_address_found(app, state, &inst.id, &profile).await;
            }
        }
        finish_remote_scan(state, &host.id);
        return InstanceState::Running;
    }

    let verdict = check_remote_instance(state, inst, host).await;
    // That machine was talked to, whatever the answer was: its instances are no
    // longer 「还没查」.
    finish_remote_scan(state, &host.id);
    let (run_host, data_dir) = match remote_parts(host, &state.data_dir) {
        Ok(parts) => parts,
        Err(reason) => return report_unreachable(app, state, inst, host, reason),
    };
    match verdict {
        RemoteVerdict::Running {
            pid,
            started_at,
            url,
            profile,
        } => {
            state.set_host_status(&host.id, HostStatus::reachable());
            let run = RemoteRun::of(host, run_host, data_dir, &inst.id, pid, started_at);
            adopt_remote_process(app, state, &inst.id, &profile, run.clone(), url.clone()).await;
            // Adopted without an address: the record that machine keeps had
            // none (DSH replaced itself and the exit path dropped it), so the
            // card would be 「运行中」 with a 打开 that cannot say where. The
            // address is still in that process's own output — look for it now,
            // at the one moment nobody is waiting on a click.
            if url.is_none()
                && repair_remote_address(state, &inst.id, &profile, &run)
                    .await
                    .is_some()
            {
                emit_address_found(app, state, &inst.id, &profile).await;
            }
            InstanceState::Running
        }
        // A port held by something that is not DSH means this instance is not
        // running; the occupant is the start path's business, not the card's.
        RemoteVerdict::Stopped | RemoteVerdict::Occupied { .. } => {
            state.set_host_status(&host.id, HostStatus::reachable());
            emit_status(
                app,
                &InstanceStatus {
                    id: inst.id.clone(),
                    state: InstanceState::Stopped,
                    url: None,
                    remote_url: None,
                    profile: inst
                        .last_profile
                        .clone()
                        .or_else(|| inst.default_profile.clone()),
                    exit_code: None,
                },
            );
            InstanceState::Stopped
        }
        RemoteVerdict::Unreachable(reason) => report_unreachable(app, state, inst, host, reason),
    }
}

/// States one host's failure on both channels the page reads: the per-host
/// status `list_statuses` consults, and the per-instance event. Reporting only
/// the returned state would leave `list_statuses` silent for that host, and a
/// silent host is one the page's `statusOf` reads as 「已停止」 — the misreading
/// ADR-0007 exists to prevent.
fn report_unreachable(
    app: &AppHandle,
    state: &AppState,
    inst: &DshInstance,
    host: &Host,
    reason: String,
) -> InstanceState {
    state.set_host_status(&host.id, HostStatus::unreachable(reason));
    emit_status(
        app,
        &InstanceStatus {
            id: inst.id.clone(),
            state: InstanceState::Unreachable,
            url: None,
            remote_url: None,
            profile: profile_hint(inst),
            exit_code: None,
        },
    );
    InstanceState::Unreachable
}

/// Marks every remote host's instances as 「还没查」 for the launch-time scan.
///
/// Until that scan has asked about a host, nothing is known about its instances,
/// and the page must not fill that gap with 「已停止」: the click that mistake
/// leads to starts a second DSH on the server (ADR-0007 后果). The page reads
/// this as 「启动中」 — a process that is being brought into the launcher's view,
/// whose 启动 button is therefore not there to press.
pub fn begin_remote_scan(state: &AppState) {
    let ids: Vec<String> = {
        let cfg = state.config.lock().unwrap();
        cfg.hosts
            .iter()
            .filter(|host| !host.builtin)
            .map(|host| host.id.clone())
            .collect()
    };
    let mut pending = state.remote_scan.lock().unwrap();
    pending.clear();
    pending.extend(ids);
}

/// One host has been asked about: its instances are no longer 「还没查」.
fn finish_remote_scan(state: &AppState, host_id: &str) {
    state.remote_scan.lock().unwrap().remove(host_id);
}

/// Adopts the instances a previous launcher session left running on remote
/// hosts — the remote half of [`adopt_surviving_instances`], which stays local
/// on purpose: this Mac's port probe can only ask about this Mac, and a local
/// process wearing the same port number is not somebody else's instance.
///
/// One pass per host, in the background, so a slow or unreachable machine holds
/// up neither the launcher nor the other machines.
pub async fn adopt_surviving_remote_instances(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (hosts, instances) = {
        let cfg = state.config.lock().unwrap();
        (cfg.hosts.clone(), cfg.instances.clone())
    };
    for host in hosts.into_iter().filter(|host| !host.builtin) {
        let mine: Vec<DshInstance> = instances
            .iter()
            .filter(|inst| inst.host_id == host.id)
            .cloned()
            .collect();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            for inst in &mine {
                // A start (or a previous session's entry) owns it already.
                if state.running.lock().await.contains_key(&inst.id) {
                    continue;
                }
                settle_remote_instance(&app, &state, inst, &host).await;
            }
            finish_remote_scan(&state, &host.id);
        });
    }
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
use crate::remote::StopOutcome;

/// Whether a profile is a DSH web application: its package.json
/// `dsh.profile.bundles` includes `@deepseek-ai/dsh-web-app`. Such profiles
/// understand `--host/--port` and get a webview; they must bind a random
/// free port so several instances don't collide.
fn is_web_profile(home_path: &std::path::Path, profile: &str) -> bool {
    let pkg = crate::profile::profile_dir(home_path, profile)
        .ok()
        .map(|d| d.join("package.json"))
        .unwrap_or_else(|| {
            home_path
                .join("profiles")
                .join(profile)
                .join("package.json")
        });
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
            if looks_like_dsh(&owner.command, &owner.args) {
                adopt_external_process(app, state, instance_id, profile, port, &host, owner.pid)
                    .await;
                return Ok(());
            }
            return Err(format!(
                "端口 {port} 已被其他进程占用（pid {}，{}），请先释放该端口或修改实例端口",
                owner.pid,
                if owner.command.is_empty() {
                    "未知进程"
                } else {
                    &owner.command
                }
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
            remote_url: None,
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
                            if looks_like_dsh(&owner.command, &owner.args) {
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
                    remote_url: None,
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
                            remote_url: None,
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

/// The running entry for one instance, as the stop path needs it: the child's
/// kill switch, the adopted pid, the remote run, and the profile to report.
async fn running_snapshot(
    state: &State<'_, AppState>,
    instance_id: &str,
) -> Option<(Arc<Notify>, Option<Adopted>, Option<RemoteRun>, String)> {
    state.running.lock().await.get(instance_id).map(|r| {
        (
            r.kill.clone(),
            r.adopted.clone(),
            r.remote.clone(),
            r.profile.clone(),
        )
    })
}

/// Stops a running instance: signals the waiter task to kill the child and
/// waits for it to reap the process and remove the registry entry. The stop
/// is idempotent: when the entry is already gone (crashed, stopped from the
/// tray, duplicate click) a terminal status is re-emitted so a stale
/// frontend converges instead of erroring with "实例未在运行".
///
/// A remote instance is stopped **on the machine that runs it**: by pid, after
/// the recorded process start time has been checked against that machine's own
/// `/proc/<pid>/stat` (ADR-0007 §4), and only 「那台机器上看不到它了」 is reported
/// as 已停止. A machine that does not answer leaves the instance 「主机不可达」.
pub async fn stop_instance_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
) -> Result<(), String> {
    // The instance's own machine decides what 「已停止」 means, so it is looked up
    // first: an instance on another host is not this Mac's business below.
    let remote_owner = {
        let cfg = state.config.lock().unwrap();
        cfg.instances
            .iter()
            .find(|inst| inst.id == instance_id)
            .cloned()
            .and_then(|inst| {
                crate::config::resolve_host(&cfg, Some(&inst.host_id))
                    .ok()
                    .filter(|host| !host.builtin)
                    .map(|host| (inst, host.clone()))
            })
    };
    let mut entry = running_snapshot(state, instance_id).await;
    if entry.is_none() {
        if let Some((inst, host)) = remote_owner {
            // Nothing registered — but on another machine that is not the same
            // statement as it is here: this session may simply not have asked
            // yet (its scan is still running). Asking is what makes 「停止」 a
            // real action instead of a silent 「已停止」 over a running process.
            settle_remote_instance(app, state, &inst, &host).await;
            entry = running_snapshot(state, instance_id).await;
            if entry.is_none() {
                // That machine answered 「没有在跑」 (or could not be reached, which
                // was reported as such) — either way there is nothing to stop.
                return Ok(());
            }
        }
    }
    let Some((kill, adopted, remote, profile)) = entry else {
        crate::log_debug!("停止实例 {instance_id}：注册表无记录，补发 stopped 状态");
        emit_status(
            app,
            &InstanceStatus {
                id: instance_id.to_string(),
                state: InstanceState::Stopped,
                url: None,
                remote_url: None,
                profile: None,
                exit_code: None,
            },
        );
        return Ok(());
    };
    if let Some(run) = remote {
        crate::log_info!(
            "收到停止实例 {instance_id} 的请求：在那台机器上结束 pid {}（{}）",
            run.pid,
            run.host.target
        );
        return match stop_remote_run(state, &run).await {
            RemoteStop::Stopped => {
                unregister_running(
                    app,
                    state,
                    instance_id,
                    InstanceStatus {
                        id: instance_id.to_string(),
                        state: InstanceState::Stopped,
                        url: None,
                        remote_url: None,
                        profile: Some(profile),
                        exit_code: None,
                    },
                )
                .await;
                Ok(())
            }
            // The machine could not answer, so 「已停止」 is not available: the
            // instance is reported as what it is — undetermined — and the page
            // offers the retry instead of a 启动 that would create a second DSH.
            RemoteStop::Unreachable(reason) => {
                state.set_host_status(&run.host_id, HostStatus::unreachable(reason.clone()));
                emit_status(
                    app,
                    &InstanceStatus {
                        id: instance_id.to_string(),
                        state: InstanceState::Unreachable,
                        url: None,
                        remote_url: run.remote_url.clone(),
                        profile: Some(profile),
                        exit_code: None,
                    },
                );
                Err(reason)
            }
            RemoteStop::NotSignalled(reason) | RemoteStop::StillRunning(reason) => Err(reason),
            // The pid was reused, so the process the launcher started is gone:
            // an entry kept for it would claim 运行中 for something that proved
            // itself ended, and nothing but a launcher restart would clear it.
            // It exited, so the record is forgotten and the row says so.
            RemoteStop::Reused(reason) => {
                crate::log_warn!("实例 {instance_id} 的 pid 已被系统复用，按已退出处理：{reason}");
                unregister_running(
                    app,
                    state,
                    instance_id,
                    InstanceStatus {
                        id: instance_id.to_string(),
                        state: InstanceState::Stopped,
                        url: None,
                        remote_url: None,
                        profile: Some(profile),
                        exit_code: None,
                    },
                )
                .await;
                Ok(())
            }
        };
    }
    if let Some(a) = adopted {
        // Adopted external process: no child handle, terminate by pid. The
        // watcher notices the exit and emits the terminal status.
        crate::log_info!(
            "停止实例 {instance_id}：结束收养的外部进程（pid {}）",
            a.pid
        );
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

/// The profile a status talks about when the process itself has nothing to say.
fn profile_hint(inst: &DshInstance) -> Option<String> {
    inst.last_profile
        .clone()
        .or_else(|| inst.default_profile.clone())
}

/// Every instance the launcher can say something about right now.
///
/// The running table answers for instances that have a process. What is left is
/// the launch-time scan of each remote host: an instance on a host that has not
/// been asked about yet is 「启动中」, and one on a host that could not answer is
/// 「主机不可达」. Both of those live here and must keep living here (ADR-0007
/// 后果): reading either of them as 「已停止」 is exactly what makes a user press
/// 启动 and end up with a second DSH on the server.
///
/// Every status says two addresses for an instance on another machine: the one
/// that works **here** (`url`, which is what 「打开」 and the plugins page use)
/// and the one that machine printed (`remote_url`), so the page can state both
/// (ADR-0008).
pub async fn list_statuses(state: &AppState) -> Vec<InstanceStatus> {
    let (instances, pending) = {
        let cfg = state.config.lock().unwrap();
        let pending = state.remote_scan.lock().unwrap().clone();
        (cfg.instances.clone(), pending)
    };
    let running = state.running.lock().await;
    let mut out: Vec<InstanceStatus> = running
        .iter()
        .map(|(id, entry)| {
            // A live entry whose machine has since stopped answering is not
            // something the launcher can keep asserting: 「运行中」 would be a
            // claim about a machine it cannot reach.
            let lost = entry
                .remote
                .as_ref()
                .and_then(|run| state.host_unreachable_reason(&run.host_id));
            InstanceStatus {
                id: id.clone(),
                state: if lost.is_some() {
                    InstanceState::Unreachable
                } else {
                    InstanceState::Running
                },
                url: if lost.is_some() {
                    None
                } else {
                    entry.url.clone()
                },
                remote_url: entry.remote.as_ref().and_then(|run| run.remote_url.clone()),
                profile: Some(entry.profile.clone()),
                exit_code: None,
            }
        })
        .collect();
    for inst in instances {
        if inst.host_id == crate::config::LOCAL_HOST_ID
            || inst.host_id.trim().is_empty()
            || running.contains_key(&inst.id)
        {
            // 本机's absence from the running table is 「已停止」, as it has
            // always been: its own pass answers immediately and completely.
            continue;
        }
        if pending.contains(&inst.host_id) {
            out.push(InstanceStatus {
                id: inst.id.clone(),
                state: InstanceState::Starting,
                url: None,
                remote_url: None,
                profile: profile_hint(&inst),
                exit_code: None,
            });
            continue;
        }
        if state.host_unreachable_reason(&inst.host_id).is_some() {
            out.push(InstanceStatus {
                id: inst.id.clone(),
                state: InstanceState::Unreachable,
                url: None,
                remote_url: None,
                profile: profile_hint(&inst),
                exit_code: None,
            });
        }
    }
    out
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
            order.extend(
                instance_ids
                    .iter()
                    .filter(|id| id.as_str() != preferred)
                    .cloned(),
            );
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
        assert!(looks_like_dsh("node", ""));
        assert!(looks_like_dsh("Node.js", ""));
        assert!(looks_like_dsh("dsh", ""));
        assert!(looks_like_dsh("/path/to/dsh-bin", ""));
        assert!(!looks_like_dsh("Python", ""));
        assert!(!looks_like_dsh("Google Chrome", ""));
        assert!(!looks_like_dsh("", ""));
    }

    /// A DSH shipped as a single-file executable carries a name of its own, and
    /// only its command line says what it is. Judged by the name alone it reads
    /// as 「别的进程占着端口」, which is the page's 「已停止」 for an instance that
    /// is up.
    #[test]
    fn looks_like_dsh_reads_the_command_line_too() {
        let args = "/home/demo/.local/share/dsh-launcher/versions/0.2.1-rc.2/\
                    node_modules/@deepseek-ai/dsh/lib/bin.js --profile web --port 3099 --no-open";
        assert!(looks_like_dsh("MainThread", args));
        assert!(
            !looks_like_dsh("MainThread", "/usr/bin/python3 -m http.server 3099"),
            "命令行里既没有 node 也没有 dsh 就不算"
        );
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
            remote_scan: std::sync::Mutex::new(std::collections::HashSet::new()),
            forwards: Default::default(),
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

    /// The base64 one remote script's base64 field carries.
    fn base64_of(text: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
    }

    const REMOTE_DATA: &str = "/home/demo/.local/share/dsh-launcher";
    const REMOTE_HOME: &str = "/home/demo/.dsh";

    fn remote_host(id: &str) -> crate::config::Host {
        crate::config::Host {
            id: id.to_string(),
            display_name: "阿里云服务器".into(),
            ssh_target: "zjq_aliyun".into(),
            registry: None,
            extra_env: [(
                "HTTPS_PROXY".to_string(),
                "http://10.0.0.9:3128".to_string(),
            )]
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
            dir: PathBuf::from(REMOTE_DATA)
                .join("versions")
                .join("0.1.0-rc.6"),
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
            remote_scan: std::sync::Mutex::new(std::collections::HashSet::new()),
            forwards: Default::default(),
        }
    }

    fn web_profile_document_fixture() -> Vec<u8> {
        br#"{"name":"dsh-profile-web","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app"]}}}"#.to_vec()
    }

    /// A start's answers, in the order the launcher asks them: **one** multi-file
    /// read of that machine's version and profile documents, the profile's store
    /// and peer pins, that machine's toolchain, the log size, the spawn, the
    /// pid's start time, and the empty running record it completes.
    ///
    /// `profile_doc` is the profile's `package.json` on that machine; `None` is a
    /// profile this start materializes.
    fn remote_start_answers(node: &str, profile_doc: Option<&[u8]>) -> Vec<FakeAnswer> {
        const BIN: &[u8] = b"#!/usr/bin/env node\n";
        const STARTUP: &[u8] = b"const p = new Command().option('--no-open', 'x')";
        vec![
            // 1. 版本与 profile 的那几个文件，一次问完：bin.js、package.json、
            //    版本自己的 bundle、profile 的三个文档。
            FakeAnswer::stdout(crate::remote::testing::files_stdout(&[
                Some(BIN),
                profile_doc,
                Some(STARTUP),
                None,
                None,
                None,
                None,
            ])),
            // 2.-3. profile 自己那份 pnpm store 与 peer 策略：各写一次。
            FakeAnswer::Done,
            FakeAnswer::Done,
            // 4. 那台机器自己的绝对 Node。
            FakeAnswer::stdout(crate::remote::testing::environment_stdout(
                crate::remote::testing::remote_tool(node, "v26.0.0", "system"),
                crate::remote::testing::remote_tool_missing(vec![]),
                &[],
            )),
            // 5. 日志现在有多少字节（起进程之前问的）。
            FakeAnswer::stdout(marked(r#"{"offset":0}"#)),
            // 6. 脱离会话地起进程。
            FakeAnswer::Pid(4242),
            // 7. 这个 pid 自己的启动时间。
            FakeAnswer::stdout(marked(r#"{"started_at":117384720}"#)),
            // 8.-9. 那台机器上的运行记录：还没有，写一条。
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
    const READY_URL: &str =
        "http://127.0.0.1:3080/?token=JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";
    const READY_TOKEN: &str = "JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";

    fn remote_run() -> RemoteRun {
        RemoteRun {
            host_id: "host-server".into(),
            host: crate::ssh::RemoteHost {
                id: "host-server".into(),
                target: "zjq_aliyun".into(),
                socket: PathBuf::from("/launcher/ssh/abc.sock"),
            },
            remote_url: None,
            pid: 4242,
            started_at: Some(117384720),
            data_dir: PathBuf::from(REMOTE_DATA),
            log_path: format!("{REMOTE_DATA}/logs/i-remote.log"),
        }
    }

    /// A priming boot's answers, in the order the launcher asks them.
    fn remote_prime_answers() -> Vec<FakeAnswer> {
        crate::remote::testing::prime_home_answers(READY_URL)
    }

    /// A brand-new DSH_HOME on another machine has to reach the same place a
    /// local one does, and that means that machine runs DSH once against it:
    /// `profiles/web` only appears on a HOME's first run, and without it the
    /// page offers no Profile to launch with — 「启动」 is a button that does
    /// nothing.
    #[tokio::test]
    async fn a_brand_new_remote_home_gets_its_web_profile_made_there() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let fake = FakeTransport::new();
        for answer in remote_prime_answers() {
            fake.answering(answer);
        }
        let state = remote_app_state(
            fake.clone(),
            remote_host(&host_id),
            remote_instance(&host_id, None),
        );
        let cfg = state.config.lock().unwrap().clone();
        let host = cfg.hosts.iter().find(|h| h.id == host_id).unwrap().clone();
        let version = cfg.versions[0].clone();

        ensure_remote_web_profile_template(&state, &host, &version, Path::new(REMOTE_HOME))
            .await
            .unwrap();

        let calls = fake.calls();
        let spawn = calls
            .iter()
            .find_map(|call| match call {
                FakeCall::Spawn(script) => Some(script.clone()),
                _ => None,
            })
            .expect("那台机器上要先跑一次 DSH，`profiles/web` 才会出现");
        assert!(
            spawn.contains("nohup setsid"),
            "那次启动也要脱离会话：{spawn}"
        );
        assert!(spawn.contains("'--profile' 'web'"), "{spawn}");
        assert!(
            spawn.contains(&format!("'DSH_HOME={REMOTE_HOME}'")),
            "跑在那个 HOME 上才有模板：{spawn}"
        );
        assert!(
            spawn.contains("'--port' '0'"),
            "端口由那台机器自己挑，这一次启动不能去抢实例要用的端口：{spawn}"
        );

        // 模板：`profiles/web` 复制成 `profiles/__temp__`（在那台机器上）。
        let copied = calls.iter().any(|call| match call {
            FakeCall::Run(script) => {
                script.contains(&format!("dsh_from='{REMOTE_HOME}/profiles/web'"))
                    && script.contains(&format!("dsh_to='{REMOTE_HOME}/profiles/__temp__'"))
            }
            _ => false,
        });
        assert!(copied, "模板要在那台机器上复制出来：{calls:?}");

        // 模板里不能留下这次启动钉住的端口，否则从它派生出的每个 Profile 都会
        // 去抢同一个端口，并且压过启动器给的 `--port`。
        let written = last_write_of(
            &fake,
            &format!("{REMOTE_HOME}/profiles/__temp__/cordis.patch.yml"),
        );
        let document = String::from_utf8_lossy(&written);
        assert!(document.contains("port: 0"), "{document}");
        assert!(
            !document.contains(crate::remote::testing::PRIME_PINNED_PORT),
            "{document}"
        );
    }

    /// A HOME that already has a profile is the user's own: the launcher does not
    /// run DSH against it, does not copy anything, and cannot be made to touch it
    /// by creating an instance on that host.
    #[tokio::test]
    async fn a_remote_home_that_already_has_profiles_is_left_alone() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let fake =
            FakeTransport::new().answering(FakeAnswer::stdout(marked(r#"{"profiles":["web"]}"#)));
        let state = remote_app_state(
            fake.clone(),
            remote_host(&host_id),
            remote_instance(&host_id, None),
        );
        let cfg = state.config.lock().unwrap().clone();
        let host = cfg.hosts.iter().find(|h| h.id == host_id).unwrap().clone();
        let version = cfg.versions[0].clone();

        ensure_remote_web_profile_template(&state, &host, &version, Path::new(REMOTE_HOME))
            .await
            .unwrap();

        assert_eq!(
            fake.calls().len(),
            1,
            "只问了一句「有什么 Profile」就够了：{:?}",
            fake.calls()
        );
    }

    /// A profile whose own document is not on that machine yet is one **this**
    /// start materializes — and a start that is given no port falls back to
    /// DSH's own default (3080), which is exactly the port another service may
    /// already hold there. So the launcher's port policy applies to this start
    /// too: the intended port (`0` = that machine's kernel picks a free one, or
    /// the pinned port, whose occupant the preflight already names), never DSH's
    /// default.
    ///
    /// From the page this can only happen on another machine: the profile list
    /// 本机 offers is the directory listing, so a name with no directory behind
    /// it is not something the page can send.
    #[tokio::test]
    async fn a_start_on_a_profile_that_is_not_materialized_yet_still_carries_the_port() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let node = "/usr/bin/node";
        let fake = FakeTransport::new();
        // 那个 profile 在那台机器上还没有自己的 package.json。
        let answers = remote_start_answers(node, None);
        for answer in answers {
            fake.answering(answer);
        }
        let state = remote_app_state(
            fake.clone(),
            remote_host(&host_id),
            remote_instance(&host_id, None),
        );
        let cfg = state.config.lock().unwrap().clone();
        let host = cfg.hosts.iter().find(|h| h.id == host_id).unwrap().clone();
        let inst = cfg.instances[0].clone();
        let version = cfg.versions[0].clone();

        spawn_remote_instance(&state, &cfg, &host, &inst, &version, "web")
            .await
            .unwrap();

        let script = fake
            .calls()
            .into_iter()
            .find_map(|call| match call {
                FakeCall::Spawn(script) => Some(script),
                _ => None,
            })
            .expect("启动要起一个脱离会话的进程");
        assert!(
            script.contains("'--port' '0'"),
            "不能让它落到 DSH 自己的 3080 上：{script}"
        );
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
        for answer in remote_start_answers(node, Some(&web_profile_document_fixture())) {
            fake.answering(answer);
        }
        let state = remote_app_state(
            fake.clone(),
            remote_host(&host_id),
            remote_instance(&host_id, Some(3080)),
        );
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
        assert!(
            script.contains("'--no-open'"),
            "那台机器的 bundle 认这个 flag：{script}"
        );
        assert!(
            !script.contains("--host"),
            "绑定地址归 profile 层：{script}"
        );
        // The environment: DSH_HOME, the instance's name, its own variables, and
        // the host record's variables.
        assert!(
            script.contains(&format!("'DSH_HOME={REMOTE_HOME}'")),
            "{script}"
        );
        assert!(
            script.contains("'DSH_LAUNCHER_INSTANCE=服务器实例'"),
            "{script}"
        );
        assert!(script.contains("'DSH_TELEMETRY_DISABLED=1'"), "{script}");
        assert!(
            script.contains("'HTTPS_PROXY=http://10.0.0.9:3128'"),
            "远端的出口由主机记录决定：{script}"
        );
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
        for answer in remote_start_answers("/usr/bin/node", Some(&web_profile_document_fixture())) {
            fake.answering(answer);
        }
        let state = remote_app_state(
            fake.clone(),
            remote_host(&host_id),
            remote_instance(&host_id, Some(3080)),
        );
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
        assert_eq!(entry["token"], READY_TOKEN, "token 必须落到记录里：{entry}");
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
        assert_eq!(
            second.url.as_deref(),
            Some("http://127.0.0.1:3080/?token=abc123")
        );
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

        forget_remote_run(&state, "i-remote", &remote_run())
            .await
            .unwrap();

        let written = last_write_of(&fake, &format!("{REMOTE_DATA}/running.json"));
        let doc: serde_json::Value = serde_json::from_slice(&written).unwrap();
        assert!(doc.get("i-remote").is_none(), "{doc}");
        assert_eq!(
            doc["i-other"]["url"], "http://127.0.0.1:1/?token=keep",
            "{doc}"
        );
    }

    /// A port this Mac has free right now, for a remote instance to have used.
    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    /// The remote addresses a run asked for a forward to, in order.
    fn forwarded(fake: &FakeTransport) -> Vec<(String, u16)> {
        fake.calls()
            .into_iter()
            .filter_map(|call| match call {
                FakeCall::OpenForward { forward, .. } => {
                    Some((forward.remote_host, forward.remote_port))
                }
                _ => None,
            })
            .collect()
    }

    /// What 「打开」 needs for a remote instance: the address **this Mac** can
    /// reach. The instance printed its own address from where it stands, which
    /// on this Mac is a different service — so the launcher forwards that port
    /// and hands over the same address on loopback, token and path unchanged.
    #[tokio::test]
    async fn a_remote_instances_open_hands_over_the_address_on_this_mac() {
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let port = free_port();
        let remote_url = format!("http://127.0.0.1:{port}/?token={READY_TOKEN}");
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        let mut run = remote_run();
        run.remote_url = Some(remote_url.clone());
        state.running.lock().await.insert(
            "i-remote".into(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: "web".into(),
                url: None,
                adopted: None,
                remote: Some(run),
            },
        );

        let opened = reachable_url_for(&state, "i-remote").await.unwrap();

        assert_eq!(opened, remote_url, "换掉的是本机地址，token 与路径原样");
        assert_eq!(
            forwarded(&fake),
            vec![("127.0.0.1".to_string(), port)],
            "转发目标就是运行记录里的地址"
        );
    }

    /// The launch-time scan and the ready path hold a remote instance the same
    /// way: the address that machine's record carries is the forward's target, and
    /// what this Mac gets is the same address on loopback — so 「打开」 after a
    /// launcher restart lands on that machine's instance, logged in, rather than
    /// on whatever else answers on this Mac at that port.
    #[tokio::test]
    async fn a_recorded_remote_address_is_forwarded_for_this_mac() {
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        let port = free_port();
        let remote_url = format!("http://127.0.0.1:{port}/?token={READY_TOKEN}");

        let local = local_address_for(&state, "i-remote", &remote_run().host, &remote_url)
            .await
            .expect("转发建起来了就该给出本机地址");

        assert_eq!(local, remote_url, "换掉的是地址的主人，token 与路径原样");
        assert_eq!(
            forwarded(&fake),
            vec![("127.0.0.1".to_string(), port)],
            "转发目标就是运行记录里的地址"
        );
    }

    /// A forward that will not come up costs the address, not the instance: it is
    /// running on that machine either way, and the next 打开 builds the forward
    /// again and says why if it still cannot. So this answers with no address
    /// rather than an error a caller would have to drop the instance over.
    #[tokio::test]
    async fn a_forward_that_will_not_come_up_costs_only_the_address() {
        let fake = FakeTransport::new().answering(FakeAnswer::fail(
            crate::ssh::HostErrorKind::Unreachable,
            "ssh: connect to host zjq_aliyun port 22: Connection refused",
        ));
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        let remote_url = format!("http://127.0.0.1:{}/?token={READY_TOKEN}", free_port());

        assert!(
            local_address_for(&state, "i-remote", &remote_run().host, &remote_url)
                .await
                .is_none(),
        );
    }

    /// 本机's path is untouched: an instance running here is already reachable
    /// here, so 「打开」 hands over its own URL and asks for no forward.
    #[tokio::test]
    async fn a_local_instance_opens_at_its_own_address_without_a_forward() {
        let fake = FakeTransport::new();
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        let url = format!("http://127.0.0.1:{}/?token={READY_TOKEN}", free_port());
        state.running.lock().await.insert(
            "i-local".into(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: "web".into(),
                url: Some(url.clone()),
                adopted: None,
                remote: None,
            },
        );

        assert_eq!(reachable_url_for(&state, "i-local").await.unwrap(), url);
        assert!(
            forwarded(&fake).is_empty(),
            "本机不需要转发：{:?}",
            fake.calls()
        );
    }

    /// The board states both addresses for a remote instance: the one that
    /// works from here (what 「打开」 uses, and what the plugins page talks to)
    /// and where the instance really is, so nobody thinks DSH is bound here.
    #[tokio::test]
    async fn the_status_of_a_remote_instance_carries_both_addresses() {
        let remote_url = format!("http://127.0.0.1:{}/?token={READY_TOKEN}", free_port());
        let local_url = format!("http://127.0.0.1:41000/?token={READY_TOKEN}");
        let state = app_state(Vec::new(), Vec::new());
        let mut run = remote_run();
        run.remote_url = Some(remote_url.clone());
        state.running.lock().await.insert(
            "i-remote".into(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: "web".into(),
                url: Some(local_url.clone()),
                adopted: None,
                remote: Some(run),
            },
        );

        let statuses = list_statuses(&state).await;

        let status = statuses.iter().find(|s| s.id == "i-remote").unwrap();
        assert_eq!(status.url.as_deref(), Some(local_url.as_str()));
        assert_eq!(status.remote_url.as_deref(), Some(remote_url.as_str()));
    }

    /// DSH replacing itself in place keeps the pid the launcher watches but
    /// moves its output to a file of its own making, so the log the launcher was
    /// following says nothing more and the running record has no address. 「打开」
    /// then has to find the address where it actually is: that machine's own
    /// file descriptors, with the token checked there before it is believed.
    #[tokio::test]
    async fn an_open_instance_without_an_address_finds_it_in_the_process_own_output() {
        // 3080 is the authority `REMOTE_COOKIE` was computed for, so the check
        // here is the same one the real machine makes: the cookie names the host
        // and port the request was made to.
        let printed = format!("dsh web: {READY_URL}\n");
        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(marked(&format!(
                r#"{{"path":"/tmp/dsh-market-restart.out.log","data":"{}"}}"#,
                base64_of(&printed)
            ))))
            .answering(FakeAnswer::stdout(marked(&format!(
                r#"{{"exists":false,"data":"{}"}}"#,
                base64_of("")
            ))))
            .answering(FakeAnswer::stdout(marked(&format!(
                r#"{{"status":303,"cookie":"{REMOTE_COOKIE}"}}"#
            ))))
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(FakeAnswer::Done)
            .answering(FakeAnswer::Done)
            .answering(FakeAnswer::Done);
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        state.running.lock().await.insert(
            "i-remote".into(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: "web".into(),
                url: None,
                adopted: None,
                remote: Some(remote_run()),
            },
        );

        let opened = reachable_url_for(&state, "i-remote").await.unwrap();

        assert_eq!(
            opened.split('/').nth(3),
            READY_URL.split('/').nth(3),
            "打开的是那台机器上真正在听的那个地址"
        );
        assert!(opened.contains(&format!("token={READY_TOKEN}")), "{opened}");
        // The fake's forward binds nothing, so the tunnel re-opens it on the way
        // out; what matters is that every forward it asked for aims at the
        // address the search found, never at anything else.
        let targets = forwarded(&fake);
        assert!(!targets.is_empty(), "打开之前要建转发");
        assert!(
            targets
                .iter()
                .all(|target| target == &("127.0.0.1".to_string(), 3080)),
            "转发目标就是找回的地址：{targets:?}"
        );
        let written = fake
            .calls()
            .into_iter()
            .find_map(|call| match call {
                FakeCall::WriteFile(path, bytes) => Some((path, bytes)),
                _ => None,
            })
            .expect("找回的地址要写进那台机器的运行记录");
        assert_eq!(written.0, format!("{REMOTE_DATA}/running.json"));
        let record = String::from_utf8(written.1).unwrap();
        assert!(
            record.contains(READY_TOKEN),
            "记录里要带上启动 token：{record}"
        );
        assert!(
            record.contains("\"pid\": 4242"),
            "记录要指向这个进程：{record}"
        );
    }

    /// The page's 重试 asks that machine again — and for an instance this session
    /// already holds, the thing worth asking again is the address it is missing.
    /// A retry that returned 「运行中」 while 「打开」 still refused would be the
    /// button that does nothing.
    #[tokio::test]
    async fn an_instance_without_an_address_is_repaired_in_place() {
        let printed = format!("dsh web: {READY_URL}\n");
        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(marked(&format!(
                r#"{{"path":"/tmp/dsh-market-restart.out.log","data":"{}"}}"#,
                base64_of(&printed)
            ))))
            .answering(FakeAnswer::stdout(marked(&format!(
                r#"{{"exists":false,"data":"{}"}}"#,
                base64_of("")
            ))))
            .answering(FakeAnswer::stdout(marked(&format!(
                r#"{{"status":303,"cookie":"{REMOTE_COOKIE}"}}"#
            ))))
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(FakeAnswer::Done)
            .answering(FakeAnswer::Done);
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        let run = remote_run();
        state.running.lock().await.insert(
            "i-remote".into(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: "web".into(),
                url: None,
                adopted: None,
                remote: Some(run.clone()),
            },
        );

        let found = repair_remote_address(&state, "i-remote", "web", &run).await;

        assert_eq!(found.as_deref(), Some(READY_URL), "地址补回来了");
        let entry = state.running.lock().await;
        let entry = entry.get("i-remote").unwrap();
        assert_eq!(
            entry.remote.as_ref().and_then(|r| r.remote_url.as_deref()),
            Some(READY_URL),
            "那条运行记录也带上了地址"
        );
        assert!(
            entry
                .url
                .as_deref()
                .map(|u| u.contains(&format!("token={READY_TOKEN}")))
                .unwrap_or(false),
            "本机可达的地址也补回来了：{:?}",
            entry.url
        );
    }

    /// Nothing to search, or nothing the instance accepts: the honest answer is
    /// 「不知道地址」, not 「没在运行」 — the second sentence would send the user to
    /// start a second DSH on a machine that is running one.
    #[tokio::test]
    async fn an_instance_whose_address_cannot_be_found_says_so_instead_of_not_running() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(marked(r#"{"path":null,"data":""}"#)))
            .answering(FakeAnswer::stdout(marked(r#"{"exists":false,"data":""}"#)));
        let state = AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        };
        state.running.lock().await.insert(
            "i-remote".into(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: "web".into(),
                url: None,
                adopted: None,
                remote: Some(remote_run()),
            },
        );

        let err = reachable_url_for(&state, "i-remote").await.unwrap_err();

        assert!(err.contains("不知道它的地址"), "{err}");
        assert!(!err.contains("未在运行"), "它明明在跑：{err}");
        assert!(
            forwarded(&fake).is_empty(),
            "没有地址就没有转发：{:?}",
            fake.calls()
        );
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
                tokio::time::timeout(
                    std::time::Duration::from_millis(300),
                    remote_kill.notified()
                )
                .await
                .is_err(),
                "远端实例不该在退出启动器时收到信号"
            );
        });
    }

    // -----------------------------------------------------------------------
    // Stopping, checking and re-adopting remote instances
    //
    // The transport double is the machine: every test here queues what that
    // machine would answer and then asserts on the conclusion the launcher
    // reached and on what it asked for — never on which of its own functions
    // ran. The Tauri side of these paths (the status event, the tray) has no
    // double and is not driven here.
    // -----------------------------------------------------------------------

    /// The cookie name that machine's instance mints for `127.0.0.1:3080` —
    /// `dsh-auth-<base64url(sha256(authority))>`, computed independently of the
    /// code under test.
    const REMOTE_COOKIE: &str =
        "dsh-auth-VPhEEcLKeqRDBoBalzN2Nm7CnfxKhLE00pKIDWxt1sw=v1.payload.sig";

    /// An `AppState` whose only machine is the transport double.
    fn transport_state(fake: &std::sync::Arc<FakeTransport>) -> AppState {
        AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        }
    }

    /// That machine's own running record for the fixture instance, as the
    /// launcher reads it back: url (token and all), profile, pid, start time.
    fn record_doc(url: Option<&str>) -> String {
        serde_json::json!({
            "i-remote": {
                "url": url,
                "token": READY_TOKEN,
                "profile": "web",
                "pid": 4242,
                "started_at": 117384720,
            }
        })
        .to_string()
    }

    fn record_answer(url: Option<&str>) -> FakeAnswer {
        FakeAnswer::stdout(crate::remote::testing::file_stdout(
            record_doc(url).as_bytes(),
        ))
    }

    fn occupied_answer(pid: u32, command: &str) -> FakeAnswer {
        FakeAnswer::stdout(marked(&format!(
            r#"{{"occupied":true,"pid":{pid},"command":"{command}","args":""}}"#
        )))
    }

    /// The same answer from a machine where DSH runs as a single-file
    /// executable: the name says nothing, the command line says everything.
    fn occupied_answer_with_args(pid: u32, command: &str, args: &str) -> FakeAnswer {
        FakeAnswer::stdout(marked(&format!(
            r#"{{"occupied":true,"pid":{pid},"command":"{command}","args":"{args}"}}"#
        )))
    }

    /// One machine, one instance, and the record it left behind — the three
    /// things every check needs.
    fn remote_case(
        fake: &std::sync::Arc<FakeTransport>,
        host_id: &str,
        port: Option<u16>,
    ) -> (AppState, DshInstance, Host) {
        let state = remote_app_state(
            fake.clone(),
            remote_host(host_id),
            remote_instance(host_id, port),
        );
        let cfg = state.config.lock().unwrap().clone();
        let inst = cfg.instances[0].clone();
        let host = cfg.hosts.iter().find(|h| h.id == host_id).unwrap().clone();
        (state, inst, host)
    }

    /// The whole stop, from the identity check to the confirmation: the signal
    /// goes to the **process group** a detached spawn leads, and 「那台机器上看
    /// 不到它了」 is the only thing that counts as stopped.
    #[tokio::test]
    async fn a_remote_stop_signals_the_group_and_confirms_before_reporting_stopped() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(
            r#"{"outcome":"stopped","started_at":117384720}"#,
        )));
        let state = transport_state(&fake);

        let outcome = stop_remote_run(&state, &remote_run()).await;

        assert_eq!(outcome, RemoteStop::Stopped);
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "核对、发信号、等它走是同一趟：{calls:?}");
        match &calls[0] {
            FakeCall::Run(script) => {
                assert!(script.contains("kill -TERM -- \"-$dsh_pid\""), "{script}");
                assert!(script.contains("dsh_started=117384720"), "{script}");
            }
            other => panic!("停止要发的是那一段脚本：{other:?}"),
        }
    }

    /// A pid number is not an identity. When the start time on that machine is
    /// not the one recorded at spawn, the pid now belongs to somebody else — and
    /// nothing may be signalled.
    #[tokio::test]
    async fn a_pid_whose_start_time_does_not_match_is_never_signalled() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(
            r#"{"outcome":"reused","started_at":999}"#,
        )));
        let state = transport_state(&fake);

        let outcome = stop_remote_run(&state, &remote_run()).await;

        match outcome {
            RemoteStop::Reused(reason) => assert!(reason.contains("复用"), "{reason}"),
            other => panic!("一个被复用的 pid 不能当成我们的进程：{other:?}"),
        }
        // 「不发信号」 is the machine's own decision, made in the same shell that
        // would have signalled: the script compares the start times first.
        let script = match &fake.calls()[0] {
            FakeCall::Run(script) => script.clone(),
            other => panic!("{other:?}"),
        };
        let check = script.find("dsh_current\" != \"$dsh_started\"").unwrap();
        let signal = script.find("kill -TERM").unwrap();
        assert!(check < signal, "核对要在发信号之前：{script}");
    }

    /// A machine that cannot be asked is not a stopped instance. Saying 「已停止」
    /// here is what makes a user start a second DSH on a server that was running
    /// one all along (ADR-0007 后果).
    #[tokio::test]
    async fn a_machine_that_does_not_answer_makes_the_stop_unreachable_not_stopped() {
        let fake = FakeTransport::new().answering(FakeAnswer::fail(
            crate::ssh::HostErrorKind::Unreachable,
            "ssh 连不上",
        ));
        let state = transport_state(&fake);

        let outcome = stop_remote_run(&state, &remote_run()).await;

        assert!(matches!(outcome, RemoteStop::Unreachable(_)), "{outcome:?}");
        assert!(
            !fake
                .calls()
                .iter()
                .any(|call| matches!(call, FakeCall::StopGroup(_))),
            "问不到那台机器时不能盲发信号：{:?}",
            fake.calls()
        );
    }

    /// ADR-0007 §2: a **pinned** port is evidence by itself — a DSH-like process
    /// listening on it is this instance, token or no token.
    #[tokio::test]
    async fn a_pinned_port_with_a_dsh_like_listener_is_adopted() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(occupied_answer(4242, "node"))
            .answering(FakeAnswer::stdout(marked(r#"{"started_at":117384720}"#)));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, Some(3080));

        let verdict = check_remote_instance(&state, &inst, &host).await;

        match verdict {
            RemoteVerdict::Running {
                pid,
                started_at,
                url,
                ..
            } => {
                assert_eq!(pid, 4242, "认领的 pid 来自那台机器的端口占用者");
                assert_eq!(started_at, Some(117384720), "顺手记下它的启动时间");
                assert_eq!(url, None, "钉死端口不要求有 token");
            }
            other => panic!("钉死的端口上有 DSH 样的进程就该认领：{other:?}"),
        }
    }

    /// The same rule on a machine whose DSH runs as a single-file executable:
    /// the process name is its own (`MainThread`), so only the command line can
    /// say this listener is DSH. Judged by the name alone, an instance that is
    /// up reads as 「已停止」 and the next start refuses the port.
    #[tokio::test]
    async fn a_pinned_port_with_a_single_file_dsh_is_adopted_too() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(occupied_answer_with_args(
                4242,
                "MainThread",
                "/home/demo/.local/share/dsh-launcher/versions/0.2.1-rc.2/\
                 node_modules/@deepseek-ai/dsh/lib/bin.js --profile web --port 3099 --no-open",
            ))
            .answering(FakeAnswer::stdout(marked(r#"{"started_at":117384720}"#)));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, Some(3080));

        let verdict = check_remote_instance(&state, &inst, &host).await;

        match verdict {
            RemoteVerdict::Running { pid, .. } => assert_eq!(pid, 4242),
            other => panic!("命令行说明它是 DSH 就该认领：{other:?}"),
        }
    }

    /// The other half of the pinned-port rule: an occupant that is not DSH is
    /// named, because that is what the user has to move before the instance can
    /// start on that port.
    #[tokio::test]
    async fn a_pinned_port_held_by_something_else_names_the_occupant() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(occupied_answer(731, "nginx"));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, Some(3080));

        let verdict = check_remote_instance(&state, &inst, &host).await;

        assert_eq!(
            verdict,
            RemoteVerdict::Occupied {
                pid: Some(731),
                command: "nginx".to_string()
            }
        );
    }

    /// A **random** port says nothing on its own, so it has to prove itself with
    /// the token that machine wrote down — and the proof is made **there**: a
    /// remote instance listens on that machine's own loopback, so the same URL
    /// asked from this Mac would reach a different service.
    #[tokio::test]
    async fn a_random_port_is_adopted_when_the_recorded_token_is_accepted_there() {
        let fake = FakeTransport::new()
            .answering(record_answer(Some(READY_URL)))
            .answering(occupied_answer(4242, "node"))
            .answering(FakeAnswer::stdout(marked(&format!(
                r#"{{"status":303,"cookie":"{REMOTE_COOKIE}"}}"#
            ))))
            .answering(FakeAnswer::stdout(marked(r#"{"started_at":117384720}"#)));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, None);

        let verdict = check_remote_instance(&state, &inst, &host).await;

        match verdict {
            RemoteVerdict::Running { pid, url, .. } => {
                assert_eq!(pid, 4242);
                assert_eq!(url.as_deref(), Some(READY_URL), "token 活着，接口就能管它");
            }
            other => panic!("token 被接受就该认领：{other:?}"),
        }
        let scripts: Vec<String> = fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                FakeCall::Run(script) => Some(script),
                _ => None,
            })
            .collect();
        assert!(
            scripts
                .iter()
                .any(|script| script.contains("curl") && script.contains(READY_TOKEN)),
            "认证必须在跑着它的那台机器上做：{scripts:?}"
        );
    }

    /// The same port, the same DSH-like listener, and a token that is no longer
    /// accepted: the process on it is not *this* instance, so it is not adopted
    /// (the local rule, asked of that machine).
    #[tokio::test]
    async fn a_random_port_whose_token_is_refused_is_not_adopted() {
        let fake = FakeTransport::new()
            .answering(record_answer(Some(READY_URL)))
            .answering(occupied_answer(4242, "node"))
            .answering(FakeAnswer::stdout(marked(
                r#"{"status":401,"cookie":null}"#,
            )));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, None);

        let verdict = check_remote_instance(&state, &inst, &host).await;

        assert_eq!(verdict, RemoteVerdict::Stopped);
    }

    /// The machine could not be asked: nothing can be said about the instance,
    /// and 「已停止」 is not one of the available answers.
    #[tokio::test]
    async fn an_instance_on_a_machine_that_cannot_be_asked_is_unreachable() {
        let fake = FakeTransport::new().answering(FakeAnswer::fail(
            crate::ssh::HostErrorKind::Timeout,
            "ssh 在 30 秒内没有返回",
        ));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, Some(3080));

        let verdict = check_remote_instance(&state, &inst, &host).await;

        assert!(
            matches!(verdict, RemoteVerdict::Unreachable(_)),
            "{verdict:?}"
        );
    }

    /// A host whose data directory is still what the user typed (`~/dsh`, before
    /// that machine ever settled it) cannot be asked anything about its
    /// instances. The reason reaches the row as 「主机不可达」的说明, so it has to
    /// carry the way out: without the instruction to test the connection, 重试
    /// answers with the same sentence forever.
    #[tokio::test]
    async fn a_machine_whose_data_directory_is_not_settled_says_how_to_settle_it() {
        let fake = FakeTransport::new();
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, mut host) = remote_case(&fake, &host_id, Some(3080));
        host.data_dir = Some(PathBuf::from("~/dsh"));

        let verdict = check_remote_instance(&state, &inst, &host).await;

        match verdict {
            RemoteVerdict::Unreachable(reason) => {
                assert!(
                    reason.contains("~/dsh"),
                    "理由要说出现在记的是什么：{reason}"
                );
                assert!(reason.contains("测试连接"), "理由还要说怎么办：{reason}");
            }
            other => panic!("远端数据目录没 settle 就没法问那台机器：{other:?}"),
        }
        assert!(
            fake.calls().is_empty(),
            "literal 波浪号不能拿去问那台机器：{:?}",
            fake.calls()
        );
    }

    /// Until the launch-time scan has asked a machine, its instances are unknown
    /// — and unknown is reported as 「启动中」, never as 「已停止」. This is the
    /// window in which a wrong 「已停止」 used to be exactly what made the user
    /// start a second DSH on the server.
    #[tokio::test]
    async fn an_instance_whose_machine_has_not_been_asked_yet_is_not_stopped() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let state = remote_app_state(
            FakeTransport::new(),
            remote_host(&host_id),
            remote_instance(&host_id, Some(3080)),
        );
        begin_remote_scan(&state);

        let statuses = list_statuses(&state).await;

        assert_eq!(statuses.len(), 1, "{statuses:?}");
        assert_eq!(statuses[0].id, "i-remote");
        assert_eq!(
            statuses[0].state,
            InstanceState::Starting,
            "还没查不等于已停止"
        );
    }

    /// Once the machine has answered 「不可达」, its instances say so — and a
    /// running entry of a host that has since stopped answering is not a claim
    /// the launcher can keep making either.
    #[tokio::test]
    async fn instances_of_an_unreachable_host_read_as_unreachable() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let state = remote_app_state(
            FakeTransport::new(),
            remote_host(&host_id),
            remote_instance(&host_id, Some(3080)),
        );

        // After the scan, with the machine having said 「连不上」.
        finish_remote_scan(&state, &host_id);
        state.set_host_status(&host_id, HostStatus::unreachable("ssh 连不上"));
        let statuses = list_statuses(&state).await;
        assert_eq!(statuses.len(), 1, "{statuses:?}");
        assert_eq!(statuses[0].state, InstanceState::Unreachable);

        // And the same instance, believed to be running here, stops claiming
        // 运行中 about a machine that cannot be reached.
        let mut run = remote_run();
        run.host_id = host_id.clone();
        state.running.lock().await.insert(
            "i-remote".to_string(),
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: "web".into(),
                url: Some(READY_URL.into()),
                adopted: None,
                remote: Some(run),
            },
        );
        let statuses = list_statuses(&state).await;
        assert_eq!(statuses.len(), 1, "{statuses:?}");
        assert_eq!(
            statuses[0].state,
            InstanceState::Unreachable,
            "{statuses:?}"
        );

        // A machine that answers again puts it back to running.
        state.set_host_status(&host_id, HostStatus::reachable());
        let statuses = list_statuses(&state).await;
        assert_eq!(statuses[0].state, InstanceState::Running, "{statuses:?}");
        assert_eq!(statuses[0].url.as_deref(), Some(READY_URL));
    }

    /// 本机's half of the same function is untouched: an instance with no process
    /// contributes nothing, which is what the page has always read as 「已停止」,
    /// and one that is running is reported as running.
    #[tokio::test]
    async fn local_instances_keep_their_own_states() {
        let state = app_state(vec![home("h1")], vec![instance("i1", "h1")]);
        assert!(
            list_statuses(&state).await.is_empty(),
            "本机没有进程 = 已停止，这一档一直是「什么都不报」"
        );

        let state = app_state(vec![home("h1")], vec![instance("i1", "h1")]);
        state.running.lock().await.insert(
            "i1".to_string(),
            running_entry("web", Some("http://127.0.0.1:3088/?token=abc")),
        );
        let statuses = list_statuses(&state).await;
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].state, InstanceState::Running);
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
        assert!(
            recall_url(&dir, "i2").is_some(),
            "forget must be per instance"
        );
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

        let hit = running_for_profile(&state, "h1", "web", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hit.url.as_deref(), Some("http://127.0.0.1:2/?token=b"));
        assert!(!hit.adopted);

        // A different HOME never borrows another HOME's instance.
        let other = running_for_profile(&state, "h2", "web", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(other.url.as_deref(), Some("http://127.0.0.1:3/?token=c"));

        // Instance registered but not started -> no running entry.
        assert!(running_for_profile(&state, "h1", "demo", None)
            .await
            .unwrap()
            .is_none());
        // Unknown HOME is an error, distinct from "not running".
        assert!(running_for_profile(&state, "nope", "web", None)
            .await
            .is_err());
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
            running.insert(
                "i1".to_string(),
                running_entry("web", Some("http://127.0.0.1:1/?token=a")),
            );
            running.insert(
                "i2".to_string(),
                running_entry("web", Some("http://127.0.0.1:2/?token=b")),
            );
            running.insert(
                "i9".to_string(),
                running_entry("web", Some("http://127.0.0.1:3/?token=c")),
            );
        }

        // Without a preference: registration order, as before.
        let first = running_for_profile(&state, "h1", "web", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.id, "i1");

        // With one: the named instance answers, and it says which one it is.
        let preferred = running_for_profile(&state, "h1", "web", Some("i2"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preferred.id, "i2");
        assert_eq!(
            preferred.url.as_deref(),
            Some("http://127.0.0.1:2/?token=b")
        );

        // A stale id (the instance was deleted), and one belonging to another
        // HOME, only rank: the page is scoped by (HOME, Profile), so neither
        // can borrow another HOME's process or turn into an error.
        for ignored in ["nope", "i9"] {
            let hit = running_for_profile(&state, "h1", "web", Some(ignored))
                .await
                .unwrap()
                .unwrap();
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
            running.insert(
                "i1".to_string(),
                running_entry("web", Some("http://127.0.0.1:3089")),
            );
            if let Some(entry) = running.get_mut("i1") {
                entry.adopted = Some(Adopted {
                    pid: 42,
                    port: 3089,
                    host: "127.0.0.1".to_string(),
                });
            }
        }
        let hit = running_for_profile(&state, "h1", "web", None)
            .await
            .unwrap()
            .unwrap();
        assert!(hit.adopted);
        assert_eq!(hit.url.as_deref(), Some("http://127.0.0.1:3089"));
    }
}
