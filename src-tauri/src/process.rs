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

pub struct RunningInstance {
    /// Stop signal. The waiter task owns the child and `select!`s on this, so
    /// `stop_instance_process` never touches the child handle directly.
    pub kill: Arc<Notify>,
    pub profile: String,
    pub url: Option<String>,
    pub adopted: Option<Adopted>,
    pub remote: Option<RemoteRun>,
}

#[derive(Clone, Debug)]
pub struct RemoteRun {
    pub host_id: String,
    pub host: crate::ssh::RemoteHost,
    pub remote_url: Option<String>,
    /// The pid `setsid` started — also the id of the process group a stop
    /// signals as a whole.
    pub pid: i32,
    pub started_at: Option<u64>,
    pub data_dir: PathBuf,
    pub log_path: String,
}

impl RemoteRun {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRunRecord {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    pub profile: String,
    pub pid: i32,
    #[serde(default)]
    pub started_at: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Adopted {
    pub pid: i32,
    pub port: u16,
    pub host: String,
}

struct PortOwner {
    pid: i32,
    command: String,
    args: String,
}

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

fn looks_like_dsh(command: &str, args: &str) -> bool {
    let looks = |text: &str| text.to_ascii_lowercase().contains("dsh");
    looks(command) || looks(args)
}

fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill(2) with signal 0 performs the permission+existence check
    // without delivering anything.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}

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
            forget_url(&data_dir, instance_id);
            None
        }
        None => None,
    };
    let url = remembered.clone().unwrap_or(bare);
    {
        let mut running = state.running.lock().await;
        if running.contains_key(instance_id) {
            return;
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

fn port_of_url(url: &str) -> Option<u16> {
    reqwest::Url::parse(url.trim()).ok()?.port()
}

pub async fn adopt_surviving_instances(app: &AppHandle) {
    let state = app.state::<AppState>();
    let data_dir = state.data_dir.clone();
    struct Candidate {
        id: String,
        profile: String,
        host: String,
        port: u16,
        remembered: Option<String>,
        pinned: bool,
    }
    let candidates: Vec<Candidate> = {
        let cfg = state.config.lock().unwrap();
        cfg.instances
            .iter()
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

pub(crate) fn forget_url(data_dir: &std::path::Path, instance_id: &str) {
    let mut map = read_remembered(data_dir);
    if map.remove(instance_id).is_some() {
        write_remembered(data_dir, &map);
    }
}

pub(crate) fn recall_url(data_dir: &std::path::Path, instance_id: &str) -> Option<String> {
    read_remembered(data_dir)
        .get(instance_id)
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub(crate) fn recall_profile(data_dir: &std::path::Path, instance_id: &str) -> Option<String> {
    read_remembered(data_dir)
        .get(instance_id)
        .and_then(|v| v.get("profile"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub(crate) fn manageable_url(entry: &RunningEntry) -> Option<&str> {
    entry.url.as_deref().filter(|u| u.contains("token="))
}

const NOT_RUNNING: &str = "实例未在运行或尚未就绪";

const ADDRESS_UNKNOWN: &str = "实例在那台机器上运行，但启动器不知道它的地址（DSH 自己重启后启动 token 没能找回来）。点「重试」再找一次；还找不到就从启动器重启这个实例。";

/// How much of a process's own output the launcher reads when it has to find the
/// address such an instance printed. Bounded on purpose: the line being looked
/// for is the newest one, and a long-lived instance's log can be gigabytes.
const ADDRESS_SEARCH_BYTES: usize = 64 * 1024;

/// How many of those addresses may be spent asking that machine before the
/// answer is 「不知道」. One restart prints a handful; a long-lived instance that
/// restarted a dozen times does not deserve a dozen ssh round trips.
const ADDRESS_CANDIDATES: usize = 8;

fn urls_in(text: &str) -> Vec<String> {
    text.lines().filter_map(url_in).collect()
}

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

const REMOTE_TAIL_WAIT_SECS: u64 = 10;

/// How long to wait before reading a remote log again after the machine could
/// not answer. A machine that went away comes back; the follower is what notices
/// either way, so a failed read is not the end of the watch.
const REMOTE_FOLLOW_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

fn remote_record_path(data_dir: &Path) -> String {
    data_dir.join("running.json").to_string_lossy().to_string()
}

/// Reading that file, changing one entry and writing it back is two round trips,
/// so two instances starting at once could drop each other's entry. One lock for
/// every host is enough: these writes are rare and tiny.
static REMOTE_RECORD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn token_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url.trim())
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, value)| value.to_string())
}

fn url_in(line: &str) -> Option<String> {
    url_re().captures(line).map(|cap| cap[1].to_string())
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileNature {
    Web,
    NotWeb,
    Unmaterialized,
}

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

#[derive(Clone, Debug)]
pub(crate) struct RemoteStart {
    pub run: RemoteRun,
    pub offset: u64,
}

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
    let mut next = |asked: bool| -> Option<Vec<u8>> {
        if asked {
            answers.next().flatten()
        } else {
            None
        }
    };

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
    // them, `--no-open` only when that machine's bundle understands it. The same
    // resolution names the directories to put on the child's `PATH`: the DSH it
    // starts looks `pnpm` up by name for plugin management, and the ssh
    // session's own `PATH` does not reach a version-manager install.
    let (node, path_dirs) = crate::runtime::remote_spawn_tools(state, host, &run_host).await?;
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
        .spawn_detached(&run_host, &argv, &log_path, &path_dirs)
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

const REMOTE_PRIME_LOG_LINES: usize = 200;
const REMOTE_PRIME_WAIT: std::time::Duration = std::time::Duration::from_secs(90);

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
    let mut argv = vec![
        "/usr/bin/env".to_string(),
        format!("DSH_HOME={}", home_path.to_string_lossy()),
    ];
    argv.extend(crate::launch::template_boot_argv(&node, &version.dir, 0));

    let log_path = remote_prime_log_path(&data_dir, home_path);
    let offset = crate::remote::log_size(transport, &run_host, &log_path)
        .await
        .map_err(|e| format!("无法读取那台机器上的日志（{log_path}）：{e}"))?;
    let pid = state
        .host_transport
        .spawn_detached(&run_host, &argv, &log_path, &[])
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
        let mut lines = Vec::new();
        if crate::remote::take_log_lines(&mut pending, &mut lines) {
            crate::log_warn!(
                "「{}」上预热 profile 的输出有一行超过 {} 字节，已截断",
                host.display_name,
                crate::remote::MAX_LOG_LINE_BYTES
            );
        }
        for line in lines {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteLogRead {
    pub offset: u64,
    pub url: Option<String>,
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
    let mut lines = Vec::new();
    if crate::remote::take_log_lines(pending, &mut lines) {
        crate::log_warn!(
            "实例 {instance_id} 的输出有一行超过 {} 字节，已截断",
            crate::remote::MAX_LOG_LINE_BYTES
        );
    }
    let mut url = None;
    for line in &lines {
        if url.is_none() {
            url = url_in(line);
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
            crate::log_warn!("实例 {instance_id} 的远端运行记录没写上（{e}）");
        }
    }

    Ok(RemoteLogRead {
        offset: chunk.offset,
        url,
        alive: chunk.alive,
    })
}

async fn start_remote_instance(
    app: &AppHandle,
    state: &State<'_, AppState>,
    cfg: &crate::config::Config,
    host: &Host,
    inst: &DshInstance,
    version: &DshVersion,
    profile: &str,
) -> Result<(), String> {
    let start_claim = claim_start(state, &inst.id).await?;
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
            // This adoption *is* the start: the start holds the id, and the
            // instance it found on that machine is the one it would have
            // started. Registering it here is what makes 「启动」 on an
            // instance that is already running answer with 运行中 instead of
            // leaving it 启动中 with nothing in the running table.
            adopt_remote_process_with(
                app,
                state,
                &inst.id,
                &running_profile,
                RemoteRun::of(host, run_host, data_dir, &inst.id, pid, started_at),
                url,
                None,
                true,
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
        RemoteVerdict::Stopped => {}
        RemoteVerdict::Unreachable(reason) => {
            return Err(format!(
                "无法确认「{}」上的端口状态，已停止启动：{reason}",
                host.display_name
            ));
        }
    }

    let start = match spawn_remote_instance(state, cfg, host, inst, version, profile).await {
        Ok(start) => start,
        Err(e) => {
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

    if let Err(e) = register_running(
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
    .await
    {
        crate::log_warn!(
            "实例 {} 在「{}」上已拉起，但登记运行状态失败：{e}",
            inst.id,
            host.display_name
        );
        let _ = stop_remote_run(state, &start.run).await;
        return Err(e);
    }
    drop(start_claim);

    follow_remote_instance(app, &inst.id, profile, start.run, start.offset);
    Ok(())
}

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
            if !instance_recorded(&state, &id) {
                crate::log_warn!("实例 {id} 的记录已不存在，不再跟随它的远端日志");
                forget_running_entry(&state, &id, run.pid).await;
                return;
            }

            let read = read_remote_log(&state, &id, &profile, &run, offset, &mut pending).await;
            let read = match read {
                Ok(read) => read,
                Err(e) => {
                    crate::log_warn!("实例 {id} 的远端日志暂时读不到：{e}");
                    tokio::time::sleep(REMOTE_FOLLOW_RETRY).await;
                    continue;
                }
            };
            offset = read.offset;

            if let Some(remote_url) = read.url.filter(|_| !announced) {
                announced = true;
                crate::log_info!("实例 {id} 已就绪：{remote_url}");
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
                match adopt_remote_successor(&app, &state, &id).await {
                    RemoteSuccessor::Adopted => return,
                    RemoteSuccessor::Unknown => {
                        tokio::time::sleep(REMOTE_WATCH_BACKOFF).await;
                        continue;
                    }
                    RemoteSuccessor::Absent => {}
                }
                if !remote_entry_matches(&state, &id, run.pid).await {
                    return;
                }
                crate::log_info!("实例 {id} 在那台机器上已退出（pid {}）", run.pid);
                let status = InstanceStatus {
                    id: id.clone(),
                    state: InstanceState::Exited,
                    url: None,
                    remote_url: None,
                    profile: Some(profile.clone()),
                    exit_code: None,
                };
                unregister_remote_entry(&app, &state, &id, &[run.pid], status).await;
                return;
            }
        }
    });
}

const REMOTE_STOP_WAIT_SECS: u64 = 20;

const REMOTE_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
const REMOTE_WATCH_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);

const REMOTE_SUCCESSOR_ATTEMPTS: usize = 10;
const REMOTE_SUCCESSOR_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// How many replacements a stop may chase on the instance's port. DSH's restart
/// helper starts one; a second round covers a stop that raced the helper's own
/// start, and the bound is what keeps a restart loop from holding the click.
///
/// What is left after the last round is a replacement that started between the
/// last look and the click's return: no polling stop can close that window, and
/// the periodic pass adopts it, so the row returns to 运行中 rather than claiming
/// a stop that did not hold.
const REMOTE_STOP_REPLACEMENT_ROUNDS: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteVerdict {
    Running {
        pid: i32,
        started_at: Option<u64>,
        url: Option<String>,
        profile: String,
    },
    Stopped,
    /// The instance's **pinned** port is held by something that is not DSH. The
    /// instance is not running, and a start must name who is in the way instead
    /// of dying with EADDRINUSE.
    Occupied {
        pid: Option<i32>,
        command: String,
    },
    Unreachable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteStop {
    Stopped,
    NotSignalled(String),
    Reused(String),
    StillRunning(String),
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

fn remote_parts(
    host: &Host,
    launcher_data_dir: &Path,
) -> Result<(crate::ssh::RemoteHost, PathBuf), String> {
    let run_host = crate::ssh::RemoteHost::of(host, launcher_data_dir)
        .map_err(|_| format!("「{}」是本机，它上面没有远端进程", host.display_name))?;
    let data_dir = host.data_dir(launcher_data_dir)?;
    Ok((run_host, data_dir))
}

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
    let Some(port) = inst.port.or_else(|| url.as_deref().and_then(port_of_url)) else {
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

async fn adopt_remote_process(
    app: &AppHandle,
    state: &AppState,
    instance_id: &str,
    profile: &str,
    run: RemoteRun,
    remote_url: Option<String>,
    replacing: Option<i32>,
) -> bool {
    adopt_remote_process_with(
        app,
        state,
        instance_id,
        profile,
        run,
        remote_url,
        replacing,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn adopt_remote_process_with(
    app: &AppHandle,
    state: &AppState,
    instance_id: &str,
    profile: &str,
    mut run: RemoteRun,
    remote_url: Option<String>,
    replacing: Option<i32>,
    by_start: bool,
) -> bool {
    if let Some(remote) = remote_url.as_deref() {
        run.remote_url = Some(remote.to_string());
    }
    {
        let mut running = state.running.lock().await;
        let entry = RunningInstance {
            kill: Arc::new(Notify::new()),
            profile: profile.to_string(),
            url: None,
            adopted: None,
            remote: Some(run.clone()),
        };
        if !install_remote_entry(&mut running, instance_id, entry, replacing, by_start) {
            return false;
        }
    }
    // The forward is built only after the entry is ours: one opened for an
    // adoption that lost its slot would be a connection nobody reads.
    let local_url = match run.remote_url.clone() {
        Some(remote) => {
            let url = local_address_for(state, instance_id, &run.host, &remote).await;
            let mut running = state.running.lock().await;
            if let Some(entry) = running.get_mut(instance_id) {
                if entry.remote.as_ref().map(|run| run.pid) == Some(run.pid) {
                    entry.url = url.clone();
                }
            }
            url
        }
        None => None,
    };
    state.set_host_status(&run.host_id, HostStatus::reachable());
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
    true
}

fn install_remote_entry(
    running: &mut std::collections::HashMap<String, RunningInstance>,
    instance_id: &str,
    entry: RunningInstance,
    replacing: Option<i32>,
    by_start: bool,
) -> bool {
    // A start holds the id until its entry is registered. Every other adoption
    // must give way to it; the start's own preflight adoption — the one that
    // recognises an instance already running on that machine — is that start,
    // and it is the one case allowed through.
    if !by_start && start_is_in_flight(instance_id) {
        return false;
    }
    if !can_adopt_remote_entry(running.get(instance_id), replacing) {
        return false;
    }
    running.remove(instance_id);
    running.insert(instance_id.to_string(), entry);
    true
}

fn can_adopt_remote_entry(existing: Option<&RunningInstance>, replacing: Option<i32>) -> bool {
    match existing {
        None => replacing.is_none(),
        Some(entry) => match replacing {
            Some(old_pid) => entry.remote.as_ref().map(|run| run.pid) == Some(old_pid),
            None => false,
        },
    }
}

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
            if !instance_recorded(&state, &id) {
                crate::log_warn!("实例 {id} 的记录已不存在，不再观察它的远端进程");
                forget_running_entry(&state, &id, run.pid).await;
                return;
            }
            match crate::remote::alive(&*state.host_transport, &run.host, run.pid).await {
                Ok(true) => {
                    if reported_lost {
                        reported_lost = false;
                        state.set_host_status(&run.host_id, HostStatus::reachable());
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
                    match adopt_remote_successor(&app, &state, &id).await {
                        RemoteSuccessor::Adopted => return,
                        RemoteSuccessor::Unknown => {
                            interval = REMOTE_WATCH_BACKOFF;
                            reported_lost = true;
                            continue;
                        }
                        RemoteSuccessor::Absent => {}
                    }
                    if !remote_entry_matches(&state, &id, run.pid).await {
                        return;
                    }
                    crate::log_info!(
                        "实例 {id} 在「{}」上已退出（pid {}）",
                        run.host.target,
                        run.pid
                    );
                    let status = InstanceStatus {
                        id: id.clone(),
                        state: InstanceState::Exited,
                        url: None,
                        remote_url: None,
                        profile: Some(profile.clone()),
                        exit_code: None,
                    };
                    unregister_remote_entry(&app, &state, &id, &[run.pid], status).await;
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

enum RemoteSuccessor {
    Adopted,
    Absent,
    Unknown,
}

async fn await_remote_successor(
    state: &AppState,
    inst: &DshInstance,
    host: &Host,
    run: &RemoteRun,
) -> Result<Option<(i32, Option<u64>, Option<String>)>, String> {
    let (run_host, data_dir) = remote_parts(host, &state.data_dir)?;
    // No address to probe: a non-web profile has no port, and an instance that
    // never printed one left nothing to look at. That is 「无从探测」 rather than
    // 「确认不在」 — but the caller has already seen the process go, so the exit
    // stands.
    let Some(port) = inst
        .port
        .or_else(|| run.remote_url.as_deref().and_then(port_of_url))
    else {
        return Ok(None);
    };
    let transport = &*state.host_transport;
    let mut saw_dsh_without_address = false;
    for attempt in 0..REMOTE_SUCCESSOR_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(REMOTE_SUCCESSOR_INTERVAL).await;
        }
        let owner = crate::remote::port_owner(transport, &run_host, port)
            .await
            .map_err(|e| {
                format!(
                    "在「{}」上查端口 {port} 的占用者失败：{e}",
                    host.display_name
                )
            })?;
        let Some(owner) = owner else { continue };
        let Some(pid) = owner.pid else {
            return Err(format!(
                "「{}」上端口 {port} 被占用，但读不到占用者的 pid",
                host.display_name
            ));
        };
        if pid == run.pid || !looks_like_dsh(&owner.command, &owner.args) {
            continue;
        }
        let started_at = crate::remote::process_start_time(transport, &run_host, pid)
            .await
            .ok()
            .flatten();
        let successor = RemoteRun::of(
            host,
            run_host.clone(),
            data_dir.clone(),
            &inst.id,
            pid,
            started_at,
        );
        let found = find_remote_address(state, &successor).await;
        if found.is_some() || inst.port.is_some() {
            return Ok(Some((pid, started_at, found)));
        }
        saw_dsh_without_address = true;
    }
    if saw_dsh_without_address {
        Err(format!(
            "「{}」上端口 {port} 有 DSH 进程，但没能确认它的启动地址",
            host.display_name
        ))
    } else {
        Ok(None)
    }
}

async fn stop_remote_replacements(
    state: &AppState,
    instance_id: &str,
    run: &RemoteRun,
) -> Result<Vec<i32>, String> {
    let (inst, host) = {
        let cfg = state.config.lock().unwrap();
        let Some(inst) = cfg
            .instances
            .iter()
            .find(|inst| inst.id == instance_id)
            .cloned()
        else {
            return Ok(Vec::new());
        };
        let Ok(host) = crate::config::resolve_host(&cfg, Some(&inst.host_id)).cloned() else {
            return Ok(Vec::new());
        };
        (inst, host)
    };
    if host.builtin {
        return Ok(Vec::new());
    }
    let (run_host, _data_dir) = remote_parts(&host, &state.data_dir)?;
    let Some(port) = inst
        .port
        .or_else(|| run.remote_url.as_deref().and_then(port_of_url))
    else {
        return Ok(Vec::new());
    };
    let transport = &*state.host_transport;
    let mut stopped = Vec::new();
    for _ in 0..REMOTE_STOP_REPLACEMENT_ROUNDS {
        let owner = crate::remote::port_owner(transport, &run_host, port)
            .await
            .map_err(|e| {
                format!(
                    "在「{}」上查端口 {port} 的占用者失败：{e}",
                    host.display_name
                )
            })?;
        let Some(owner) = owner else { break };
        let Some(pid) = owner.pid else { break };
        if pid == run.pid || stopped.contains(&pid) || !looks_like_dsh(&owner.command, &owner.args)
        {
            break;
        }
        let started_at = crate::remote::process_start_time(transport, &run_host, pid)
            .await
            .ok()
            .flatten();
        let Some(started_at) = started_at else {
            return Err(format!(
                "实例在停止过程中被 DSH 重启了（「{}」上端口 {port} 现在是 pid {pid}），但读不到这个进程的启动时间，没有发信号。请在那台机器上核对后自行停止：ssh {} 'kill -TERM -- -{pid}'",
                host.display_name, run.host.target
            ));
        };
        match stop_remote_run(
            state,
            &RemoteRun::of(
                &host,
                run_host.clone(),
                run.data_dir.clone(),
                instance_id,
                pid,
                Some(started_at),
            ),
        )
        .await
        {
            RemoteStop::Stopped => {
                crate::log_info!(
                    "实例 {instance_id} 停止期间 DSH 重启出的进程（pid {pid}）也停掉了"
                );
                stopped.push(pid);
            }
            RemoteStop::Unreachable(reason) => return Err(reason),
            RemoteStop::NotSignalled(reason)
            | RemoteStop::StillRunning(reason)
            | RemoteStop::Reused(reason) => return Err(reason),
        }
    }
    Ok(stopped)
}

async fn adopt_remote_successor(
    app: &AppHandle,
    state: &AppState,
    instance_id: &str,
) -> RemoteSuccessor {
    let (inst, host) = {
        let cfg = state.config.lock().unwrap();
        let Some(inst) = cfg
            .instances
            .iter()
            .find(|inst| inst.id == instance_id)
            .cloned()
        else {
            return RemoteSuccessor::Absent;
        };
        let Ok(host) = crate::config::resolve_host(&cfg, Some(&inst.host_id)).cloned() else {
            return RemoteSuccessor::Absent;
        };
        (inst, host)
    };
    if host.builtin {
        return RemoteSuccessor::Absent;
    }
    let (gone, profile) = {
        let running = state.running.lock().await;
        match running.get(instance_id) {
            Some(entry) => match entry.remote.clone() {
                Some(run) => (run, entry.profile.clone()),
                None => return RemoteSuccessor::Absent,
            },
            None => return RemoteSuccessor::Absent,
        }
    };
    let (pid, started_at, remote_url) =
        match await_remote_successor(state, &inst, &host, &gone).await {
            Ok(Some(found)) => found,
            Ok(None) => return RemoteSuccessor::Absent,
            Err(reason) => {
                state.set_host_status(&host.id, HostStatus::unreachable(reason.clone()));
                emit_status(
                    app,
                    &InstanceStatus {
                        id: instance_id.to_string(),
                        state: InstanceState::Unreachable,
                        url: None,
                        remote_url: gone.remote_url.clone(),
                        profile: Some(profile),
                        exit_code: None,
                    },
                );
                return RemoteSuccessor::Unknown;
            }
        };
    let (run_host, data_dir) = match remote_parts(&host, &state.data_dir) {
        Ok(parts) => parts,
        Err(reason) => {
            state.set_host_status(&host.id, HostStatus::unreachable(reason));
            return RemoteSuccessor::Unknown;
        }
    };
    let run = RemoteRun::of(&host, run_host, data_dir, instance_id, pid, started_at);
    let adopted = adopt_remote_process(
        app,
        state,
        instance_id,
        &profile,
        run.clone(),
        remote_url.clone(),
        Some(gone.pid),
    )
    .await;
    if !adopted {
        return RemoteSuccessor::Absent;
    }
    let record = RemoteRunRecord {
        url: remote_url.clone(),
        token: remote_url.as_deref().and_then(token_of),
        profile: profile.clone(),
        pid,
        started_at,
    };
    if let Err(e) = remember_remote_run(state, instance_id, &record, &run).await {
        crate::log_warn!("实例 {instance_id} 的远端运行记录没写上（{e}）");
    }
    crate::log_info!(
        "实例 {instance_id} 在「{}」上换了新进程（pid {pid}），已自动认领",
        host.display_name
    );
    if remote_url.is_none()
        && repair_remote_address(state, instance_id, &profile, &run)
            .await
            .is_some()
    {
        emit_address_found(app, state, instance_id, &profile).await;
    }
    RemoteSuccessor::Adopted
}

/// Whether the running table still holds `pid` as this instance's remote
/// process. A watcher that spent seconds looking for a successor must ask this
/// again before projecting a terminal status: the user may have pressed 停止
/// while it was looking, and that transition already has its own status.
async fn remote_entry_matches(state: &AppState, instance_id: &str, pid: i32) -> bool {
    state
        .running
        .lock()
        .await
        .get(instance_id)
        .and_then(|entry| entry.remote.as_ref())
        .map(|run| run.pid == pid)
        .unwrap_or(false)
}

pub(crate) async fn settle_remote_instance(
    app: &AppHandle,
    state: &State<'_, AppState>,
    inst: &DshInstance,
    host: &Host,
) -> InstanceState {
    if start_is_in_flight(&inst.id) {
        finish_remote_scan(state, &host.id);
        return InstanceState::Starting;
    }
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
            state.set_host_status(&host.id, HostStatus::reachable());
        }
        finish_remote_scan(state, &host.id);
        return InstanceState::Running;
    }

    let verdict = check_remote_instance(state, inst, host).await;
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
            if start_is_in_flight(&inst.id) {
                return InstanceState::Starting;
            }
            state.set_host_status(&host.id, HostStatus::reachable());
            let run = RemoteRun::of(host, run_host, data_dir, &inst.id, pid, started_at);
            let adopted = adopt_remote_process(
                app,
                state,
                &inst.id,
                &profile,
                run.clone(),
                url.clone(),
                None,
            )
            .await;
            if !adopted {
                // Something took the id between the machine's answer and this
                // write: that owner is the truth, and this answer is stale.
                return InstanceState::Running;
            }
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
        RemoteVerdict::Stopped | RemoteVerdict::Occupied { .. } => {
            state.set_host_status(&host.id, HostStatus::reachable());
            if state.running.lock().await.contains_key(&inst.id) {
                return InstanceState::Running;
            }
            if start_is_in_flight(&inst.id) {
                return InstanceState::Starting;
            }
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

fn finish_remote_scan(state: &AppState, host_id: &str) {
    state.remote_scan.lock().unwrap().remove(host_id);
}

/// How often the periodic pass re-asks every remote host about the instances
/// this session does not hold.
///
/// The pass is a backstop, not the main path: a DSH that restarts itself while
/// the launcher is open is caught by its own watcher within seconds. What is
/// left for this pass is the instance started on a server outside the launcher,
/// and the machine that was unreachable and came back. A minute is soon enough
/// for both, and keeps a machine with stopped instances to a few calls a minute.
const REMOTE_RECONCILE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// One scan at a time: the launch-time pass and the periodic pass ask the same
/// question, and two answers racing on one instance would let a later
/// 「已停止」 overwrite a 「运行中」 that was just adopted.
static REMOTE_SCAN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn adopt_surviving_remote_instances(app: &AppHandle) {
    scan_remote_instances(app).await;
}

pub async fn watch_remote_instances(app: &AppHandle) {
    loop {
        tokio::time::sleep(REMOTE_RECONCILE_INTERVAL).await;
        scan_remote_instances(app).await;
    }
}

/// One pass over the remote hosts: every instance this session does not already
/// hold is settled against its own machine. A host's answers are one task, so an
/// unreachable machine costs its own timeout and not the other hosts'.
async fn scan_remote_instances(app: &AppHandle) {
    let _scan = REMOTE_SCAN_LOCK.lock().await;
    let state = app.state::<AppState>();
    let (hosts, instances) = {
        let cfg = state.config.lock().unwrap();
        (cfg.hosts.clone(), cfg.instances.clone())
    };
    let mut tasks = Vec::new();
    for host in hosts.into_iter().filter(|host| !host.builtin) {
        let mine: Vec<DshInstance> = instances
            .iter()
            .filter(|inst| inst.host_id == host.id)
            .cloned()
            .collect();
        let app = app.clone();
        tasks.push(tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            for inst in &mine {
                if state.running.lock().await.contains_key(&inst.id) || start_is_in_flight(&inst.id)
                {
                    continue;
                }
                settle_remote_instance(&app, &state, inst, &host).await;
            }
            finish_remote_scan(&state, &host.id);
        }));
    }
    for task in tasks {
        let _ = task.await;
    }
}

pub fn npm() -> &'static str {
    "npm"
}

pub fn node() -> &'static str {
    "node"
}

pub fn hide_console(cmd: &mut Command) -> &mut Command {
    cmd
}

pub use crate::launch::instance_env as build_env;
use crate::launch::{version_bin, version_bin_ready};
use crate::remote::StopOutcome;

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

/// Instance ids whose start is in flight. A start claims its id here before it
/// spawns anything, so two concurrent starts cannot both pass the running check
/// and leave two processes registered under one id. The claim is released on
/// every path: it lives in [`StartClaim`], whose `Drop` removes it.
fn starting_claims() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static STARTING: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        OnceLock::new();
    STARTING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

struct StartClaim {
    id: String,
}

impl Drop for StartClaim {
    fn drop(&mut self) {
        if let Ok(mut starting) = starting_claims().lock() {
            starting.remove(&self.id);
        }
    }
}

/// Whether a start for `id` is in flight, held by [`claim_start`] until the
/// running entry is registered or the start gives up. Read by the remote scan
/// so a status it took before the start registered cannot be projected over
/// the start's own status.
fn start_is_in_flight(id: &str) -> bool {
    starting_claims()
        .lock()
        .map(|starting| starting.contains(id))
        .unwrap_or(false)
}

async fn claim_start(state: &State<'_, AppState>, id: &str) -> Result<StartClaim, String> {
    {
        let mut starting = starting_claims()
            .lock()
            .map_err(|_| "启动占位已损坏".to_string())?;
        if !starting.insert(id.to_string()) {
            return Err("实例正在启动".to_string());
        }
    }
    let claim = StartClaim { id: id.to_string() };
    if state.running.lock().await.contains_key(id) {
        return Err("实例已在运行".to_string());
    }
    Ok(claim)
}

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

    let host = crate::config::resolve_host(&cfg, Some(&inst.host_id))?.clone();
    if !host.builtin {
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

    let start_claim = claim_start(state, instance_id).await?;

    let home_path = cfg
        .homes
        .iter()
        .find(|h| h.id == inst.home_id)
        .map(|h| h.path.clone());
    let is_web = home_path
        .as_deref()
        .map(|hp| is_web_profile(hp, profile))
        .unwrap_or(profile == "web");

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

    if is_web {
        if let Some(hp) = home_path.as_deref() {
            crate::profile::assert_profile_lan_bind_port(hp, profile, inst.port.unwrap_or(0));
        }
    }

    if let Some(hp) = home_path.as_deref() {
        let launcher_store = crate::toolchain::store_dir(&state.data_dir);
        match crate::profile::profile_dir(hp, profile) {
            Ok(dir) => {
                let store = crate::plugins::profile_store_target(&dir, &launcher_store);
                if let Err(e) = crate::plugins::ensure_profile_store_pin(&dir, &store) {
                    crate::log_warn!("固定 profile pnpm store 失败（{}）: {e}", dir.display());
                }
                if let Err(e) = crate::plugins::ensure_profile_npmrc(&dir) {
                    crate::log_warn!("固定 profile peer 策略失败（{}）: {e}", dir.display());
                }
            }
            Err(e) => crate::log_warn!("解析 profile 目录失败，跳过 store 固定: {e}"),
        }
    }

    let node = crate::runtime::node_for_spawn_checked(state).await?;

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

    // The instance log is opened before the process exists: a spawn whose
    // output has nowhere to go is not a launch that can be kept, so a log that
    // cannot be created is reported instead of papered over.
    let log_dir = state.data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| format!("创建实例日志失败: {e}"))?;
    let log_path = log_dir.join(format!("{instance_id}.log"));
    let log_file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| format!("创建实例日志失败: {e}"))?,
    ));

    let mut child = cmd.spawn().map_err(|e| format!("启动进程失败: {e}"))?;
    crate::log_info!("实例 {instance_id} 进程已拉起（profile: {profile}）");

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

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
    // the tray menu is a projection of the registry. A start that reaches this
    // point alone owns the id, so an existing entry is a race the child must not
    // survive: it is reaped here and the start reports why.
    if let Err(e) = register_running(
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
    .await
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
        emit_status(
            app,
            &InstanceStatus {
                id: instance_id.to_string(),
                state: InstanceState::Stopped,
                url: None,
                remote_url: None,
                profile: Some(profile.to_string()),
                exit_code: None,
            },
        );
        return Err(e);
    }
    drop(start_claim);

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
            remove_entry_quiet(&state, &waiter_id, &waiter_kill).await;
            if stopped {
                crate::log_info!("实例 {waiter_id} 已停止（exit code: {code:?}）");
            } else {
                crate::log_warn!("实例 {waiter_id} 意外退出（exit code: {code:?}）");
            }
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
            unregister_waiter_entry(
                &waiter_app,
                &state,
                &waiter_id,
                &waiter_kill,
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

pub async fn stop_instance_process(
    app: &AppHandle,
    state: &State<'_, AppState>,
    instance_id: &str,
) -> Result<(), String> {
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
            settle_remote_instance(app, state, &inst, &host).await;
            entry = running_snapshot(state, instance_id).await;
            if entry.is_none() {
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
                let mut pids = vec![run.pid];
                match stop_remote_replacements(state, instance_id, &run).await {
                    Ok(replacements) => pids.extend(replacements),
                    Err(reason) => {
                        state
                            .set_host_status(&run.host_id, HostStatus::unreachable(reason.clone()));
                        return Err(reason);
                    }
                }
                let status = InstanceStatus {
                    id: instance_id.to_string(),
                    state: InstanceState::Stopped,
                    url: None,
                    remote_url: None,
                    profile: Some(profile),
                    exit_code: None,
                };
                if unregister_remote_entry(app, state, instance_id, &pids, status).await
                    || !state.running.lock().await.contains_key(instance_id)
                {
                    Ok(())
                } else {
                    Err(format!(
                        "实例「{instance_id}」在停止过程中被 DSH 重启了，新的进程仍在运行；请再点一次「停止」"
                    ))
                }
            }
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
            RemoteStop::Reused(reason) => {
                crate::log_warn!("实例 {instance_id} 的 pid 已被系统复用，按已退出处理：{reason}");
                let mut pids = vec![run.pid];
                match stop_remote_replacements(state, instance_id, &run).await {
                    Ok(replacements) => pids.extend(replacements),
                    Err(reason) => {
                        state
                            .set_host_status(&run.host_id, HostStatus::unreachable(reason.clone()));
                        return Err(reason);
                    }
                }
                let status = InstanceStatus {
                    id: instance_id.to_string(),
                    state: InstanceState::Stopped,
                    url: None,
                    remote_url: None,
                    profile: Some(profile),
                    exit_code: None,
                };
                if unregister_remote_entry(app, state, instance_id, &pids, status).await
                    || !state.running.lock().await.contains_key(instance_id)
                {
                    Ok(())
                } else {
                    Err(format!(
                        "实例「{instance_id}」在停止过程中被 DSH 重启了，新的进程仍在运行；请再点一次「停止」"
                    ))
                }
            }
        };
    }
    if let Some(a) = adopted {
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

pub fn kill_all(state: &AppState) {
    let running = state.running.blocking_lock();
    for entry in running.values() {
        if entry.remote.is_some() {
            continue;
        }
        if let Some(a) = &entry.adopted {
            unsafe {
                libc::kill(a.pid, libc::SIGTERM);
            }
            continue;
        }
        entry.kill.notify_one();
    }
}

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

fn profile_hint(inst: &DshInstance) -> Option<String> {
    inst.last_profile
        .clone()
        .or_else(|| inst.default_profile.clone())
}

pub async fn list_statuses(state: &AppState) -> Vec<InstanceStatus> {
    let (instances, pending) = {
        let cfg = state.config.lock().unwrap();
        let pending = state.remote_scan.lock().unwrap().clone();
        (cfg.instances.clone(), pending)
    };
    let running = state.running.lock().await;
    let mut out: Vec<InstanceStatus> = running
        .iter()
        .filter(|(id, _)| instances.iter().any(|inst| &inst.id == *id))
        .map(|(id, entry)| {
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

pub struct RunningEntry {
    pub id: String,
    pub url: Option<String>,
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
        cfg.instances
            .iter()
            .filter(|i| i.home_id == home_id)
            .map(|i| i.id.clone())
            .collect()
    };
    let running = state.running.lock().await;
    let order = match prefer {
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

async fn register_running(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
    entry: RunningInstance,
) -> Result<(), String> {
    {
        let mut running = state.running.lock().await;
        if running.contains_key(id) {
            return Err("实例已在运行".to_string());
        }
        running.insert(id.to_string(), entry);
    }
    crate::tray::rebuild_tray_menu(app).await;
    Ok(())
}

async fn unregister_running(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
    status: InstanceStatus,
) {
    let remote = state.running.lock().await.remove(id).and_then(|r| r.remote);
    forget_url(&state.data_dir, id);
    if let Some(run) = remote {
        withdraw_forward(state, &run).await;
        if let Err(e) = forget_remote_run(state, id, &run).await {
            crate::log_warn!("实例 {id} 的远端运行记录没能清掉：{e}");
        }
    }
    emit_status(app, &status);
    crate::tray::rebuild_tray_menu(app).await;
}

/// Withdraws the forward an address on another machine was reached through.
/// The forward belongs to this launcher session and holds a local port; with the
/// process gone there is nothing left on the other end of it.
async fn withdraw_forward(state: &AppState, run: &RemoteRun) {
    if let Some(remote_url) = run.remote_url.as_deref() {
        crate::tunnel::withdraw(state, &run.host_id, remote_url).await;
    }
}

/// Removes one remote entry only while it still names a pid this call is about,
/// then projects the terminal status. An adoption can replace the entry between
/// the look that saw the old pid gone and this call: deleting the replacement
/// would leave a live process unregistered, and its record would be dropped
/// while it still runs. `pids` is the process this call stopped or saw go, plus
/// any replacement it stopped on the way. Returns whether this call removed the
/// entry.
async fn unregister_remote_entry(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    pids: &[i32],
    status: InstanceStatus,
) -> bool {
    let remote = {
        let mut running = state.running.lock().await;
        let ours = running
            .get(id)
            .and_then(|entry| entry.remote.as_ref())
            .map(|run| pids.contains(&run.pid))
            .unwrap_or(false);
        if !ours {
            return false;
        }
        running.remove(id).and_then(|entry| entry.remote)
    };
    forget_url(&state.data_dir, id);
    if let Some(run) = remote {
        withdraw_forward(state, &run).await;
        if let Err(e) = forget_remote_run(state, id, &run).await {
            crate::log_warn!("实例 {id} 的远端运行记录没能清掉：{e}");
        }
    }
    emit_status(app, &status);
    crate::tray::rebuild_tray_menu(app).await;
    true
}

/// Whether the launcher still holds a record for this instance. A record that
/// is gone — the instance was deleted, or its host was — leaves nothing to
/// watch: the record is what names the process, so no later session could stop
/// it either.
fn instance_recorded(state: &AppState, instance_id: &str) -> bool {
    state
        .config
        .lock()
        .unwrap()
        .instances
        .iter()
        .any(|inst| inst.id == instance_id)
}

async fn forget_running_entry(state: &AppState, instance_id: &str, pid: i32) {
    let run = {
        let mut running = state.running.lock().await;
        let ours = running
            .get(instance_id)
            .and_then(|entry| entry.remote.as_ref())
            .map(|run| run.pid == pid)
            .unwrap_or(false);
        if !ours {
            return;
        }
        running.remove(instance_id).and_then(|entry| entry.remote)
    };
    forget_url(&state.data_dir, instance_id);
    if let Some(run) = run {
        withdraw_forward(state, &run).await;
        if let Err(e) = forget_remote_run(state, instance_id, &run).await {
            crate::log_warn!("实例 {instance_id} 的远端运行记录没能清掉：{e}");
        }
    }
}

pub(crate) async fn release_host_runs(state: &AppState, host_id: &str) -> Vec<String> {
    let entries: Vec<(String, i32)> = {
        let running = state.running.lock().await;
        running
            .iter()
            .filter_map(|(id, entry)| {
                entry
                    .remote
                    .as_ref()
                    .filter(|run| run.host_id == host_id)
                    .map(|run| (id.clone(), run.pid))
            })
            .collect()
    };
    for (id, pid) in &entries {
        forget_running_entry(state, id, *pid).await;
    }
    entries.into_iter().map(|(id, _)| id).collect()
}

/// Removes this child's running entry WITHOUT projections: for the exit waiter,
/// whose terminal status depends on the adoption re-probe that follows. Only the
/// entry that still belongs to `kill` is removed, so a newer entry under the same
/// id is left untouched.
async fn remove_entry_quiet(state: &State<'_, AppState>, id: &str, kill: &Arc<Notify>) {
    let mut running = state.running.lock().await;
    let ours = running
        .get(id)
        .map(|entry| Arc::ptr_eq(&entry.kill, kill))
        .unwrap_or(false);
    if ours {
        running.remove(id);
    }
}

/// Projects one child's terminal status, but only while the registry still
/// belongs to that child (its entry present and matched by `kill`) or is
/// already empty. A newer start under the same id owns the id and stays silent.
async fn unregister_waiter_entry(
    app: &AppHandle,
    state: &State<'_, AppState>,
    id: &str,
    kill: &Arc<Notify>,
    status: InstanceStatus,
) {
    let (owned, remote) = {
        let mut running = state.running.lock().await;
        match running.get(id) {
            Some(entry) if Arc::ptr_eq(&entry.kill, kill) => {
                (true, running.remove(id).and_then(|r| r.remote))
            }
            None => (true, None),
            Some(_) => (false, None),
        }
    };
    if !owned {
        return;
    }
    forget_url(&state.data_dir, id);
    if let Some(run) = remote {
        if let Err(e) = forget_remote_run(state, id, &run).await {
            crate::log_warn!("实例 {id} 的远端运行记录没能清掉：{e}");
        }
    }
    emit_status(app, &status);
    crate::tray::rebuild_tray_menu(app).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_dsh_matches_dsh_processes_only() {
        assert!(!looks_like_dsh("node", ""));
        assert!(!looks_like_dsh("Node.js", ""));
        assert!(!looks_like_dsh("node", "/opt/foo/bin.js"));
        assert!(!looks_like_dsh("node", "/opt/bar --profile web"));
        assert!(looks_like_dsh("dsh", ""));
        assert!(looks_like_dsh("/path/to/dsh-bin", ""));
        assert!(!looks_like_dsh("Python", ""));
        assert!(!looks_like_dsh("Google Chrome", ""));
        assert!(!looks_like_dsh("", ""));
    }

    /// A DSH shipped as a single-file executable carries a name of its own, and
    /// only its command line says what it is. Judged by the name alone it reads
    /// as 「别的进程占着端口」, which is the page's 「已停止」 for an instance that
    #[test]
    fn looks_like_dsh_reads_the_command_line_too() {
        let args = "/home/demo/.local/share/dsh-launcher/versions/0.2.1-rc.2/\
                    node_modules/@deepseek-ai/dsh/lib/bin.js --profile web --port 3099 --no-open";
        assert!(looks_like_dsh("MainThread", args));
        assert!(
            !looks_like_dsh("MainThread", "/usr/bin/python3 -m http.server 3099"),
            "命令行里没有 DSH 的证据就不算"
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
        assert!(!is_web_profile(&dir, "test"));
        std::fs::write(
            profile_dir.join("package.json"),
            r#"{"name":"dsh-profile-test","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app"]}}}"#,
        )
        .unwrap();
        assert!(is_web_profile(&dir, "test"));
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
            host_transport: crate::remote::testing::idle(),
            host_statuses: std::sync::Mutex::new(std::collections::HashMap::new()),
            remote_scan: std::sync::Mutex::new(std::collections::HashSet::new()),
            forwards: Default::default(),
        }
    }

    use crate::remote::testing::{marked, FakeAnswer, FakeCall, FakeTransport};

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

    fn remote_start_answers(
        node: &str,
        pnpm: Option<&str>,
        profile_doc: Option<&[u8]>,
    ) -> Vec<FakeAnswer> {
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
            FakeAnswer::stdout(crate::remote::testing::environment_stdout(
                crate::remote::testing::remote_tool(node, "v26.0.0", "system"),
                match pnpm {
                    Some(path) => crate::remote::testing::remote_tool(path, "10.0.0", "system"),
                    None => crate::remote::testing::remote_tool_missing(vec![]),
                },
                &[],
            )),
            FakeAnswer::stdout(marked(r#"{"offset":0}"#)),
            FakeAnswer::Pid(4242),
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

    fn remote_prime_answers() -> Vec<FakeAnswer> {
        crate::remote::testing::prime_home_answers(READY_URL)
    }

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

        let copied = calls.iter().any(|call| match call {
            FakeCall::Run(script) => {
                script.contains(&format!("dsh_from='{REMOTE_HOME}/profiles/web'"))
                    && script.contains(&format!("dsh_to='{REMOTE_HOME}/profiles/__temp__'"))
            }
            _ => false,
        });
        assert!(copied, "模板要在那台机器上复制出来：{calls:?}");

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

    #[tokio::test]
    async fn a_start_on_a_profile_that_is_not_materialized_yet_still_carries_the_port() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let node = "/usr/bin/node";
        let fake = FakeTransport::new();
        let answers = remote_start_answers(node, None, None);
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

    #[tokio::test]
    async fn a_remote_start_spawns_detached_with_that_hosts_toolchain_and_environment() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let node = "/home/linuxbrew/.linuxbrew/bin/node";
        let fake = FakeTransport::new();
        for answer in remote_start_answers(
            node,
            Some("/home/demo/.local/share/pnpm/pnpm"),
            Some(&web_profile_document_fixture()),
        ) {
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
        assert!(script.contains("nohup setsid"), "{script}");
        assert!(script.contains("< /dev/null"), "{script}");
        assert!(
            script.contains(&format!("dsh_log='{REMOTE_DATA}/logs/i-remote.log'"))
                && script.contains("2>&1 < /dev/null"),
            "输出追加到那台机器上的日志：{script}"
        );
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
        assert!(
            script.contains(
                "export PATH='/home/linuxbrew/.linuxbrew/bin':'/home/demo/.local/share/pnpm':\"$PATH\""
            ),
            "让那台机器上起的 DSH 能找到 pnpm：{script}"
        );
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

    #[tokio::test]
    async fn the_remote_run_record_names_the_pid_its_start_time_and_the_profile() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let fake = FakeTransport::new();
        for answer in
            remote_start_answers("/usr/bin/node", None, Some(&web_profile_document_fixture()))
        {
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

    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

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

    #[tokio::test]
    async fn the_status_of_a_remote_instance_carries_both_addresses() {
        let remote_url = format!("http://127.0.0.1:{}/?token={READY_TOKEN}", free_port());
        let local_url = format!("http://127.0.0.1:41000/?token={READY_TOKEN}");
        let state = app_state(
            vec![remote_home("host-server")],
            vec![remote_instance("host-server", None)],
        );
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

    #[tokio::test]
    async fn an_open_instance_without_an_address_finds_it_in_the_process_own_output() {
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

    #[test]
    fn installing_a_remote_entry_respects_the_pid_it_replaces() {
        let entry_for = |pid: i32, profile: &str| {
            let mut run = remote_run();
            run.pid = pid;
            RunningInstance {
                kill: Arc::new(Notify::new()),
                profile: profile.to_string(),
                url: None,
                adopted: None,
                remote: Some(run),
            }
        };

        let mut running = std::collections::HashMap::new();
        assert!(
            install_remote_entry(
                &mut running,
                "i-remote",
                entry_for(4343, "web"),
                None,
                false
            ),
            "空槽位上的认领要装进去"
        );
        assert!(
            !install_remote_entry(
                &mut running,
                "i-remote",
                entry_for(4444, "bot"),
                None,
                false
            ),
            "已经有条目时不能再装一条"
        );
        assert!(
            install_remote_entry(
                &mut running,
                "i-remote",
                entry_for(4444, "web"),
                Some(4343),
                false
            ),
            "后继进程替换的是那个已经消失的 pid"
        );
        assert_eq!(
            running["i-remote"].remote.as_ref().map(|run| run.pid),
            Some(4444)
        );
        assert!(
            !install_remote_entry(
                &mut running,
                "i-remote",
                entry_for(5555, "web"),
                Some(4343),
                false
            ),
            "pid 对不上说明这条已经被别人接管，不能再改"
        );

        let claim = StartClaim {
            id: "i-claim".to_string(),
        };
        starting_claims()
            .lock()
            .unwrap()
            .insert("i-claim".to_string());
        let mut empty = std::collections::HashMap::new();
        assert!(
            !install_remote_entry(&mut empty, "i-claim", entry_for(6666, "web"), None, false),
            "启动占位在，别的认领要让路"
        );
        assert!(
            install_remote_entry(&mut empty, "i-claim", entry_for(6666, "web"), None, true),
            "启动自己的预检认领要装进去，否则「启动」会把已经在跑的实例丢在启动中"
        );
        drop(claim);
    }

    #[test]
    fn a_remote_successor_replaces_only_the_entry_whose_pid_is_gone() {
        let mut run = remote_run();
        run.pid = 4242;
        let entry = RunningInstance {
            kill: Arc::new(Notify::new()),
            profile: "web".into(),
            url: None,
            adopted: None,
            remote: Some(run),
        };
        assert!(can_adopt_remote_entry(Some(&entry), Some(4242)));
        assert!(!can_adopt_remote_entry(Some(&entry), Some(999)));
        assert!(!can_adopt_remote_entry(Some(&entry), None));
        assert!(can_adopt_remote_entry(None, None));
        assert!(
            !can_adopt_remote_entry(None, Some(4242)),
            "用户已经停止之后不要再把它拉回运行中"
        );
    }

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

    /// The cookie name that machine's instance mints for `127.0.0.1:3080` —
    /// `dsh-auth-<base64url(sha256(authority))>`, computed independently of the
    /// code under test.
    const REMOTE_COOKIE: &str =
        "dsh-auth-VPhEEcLKeqRDBoBalzN2Nm7CnfxKhLE00pKIDWxt1sw=v1.payload.sig";

    fn transport_state(fake: &std::sync::Arc<FakeTransport>) -> AppState {
        AppState {
            host_transport: fake.clone(),
            ..app_state(Vec::new(), Vec::new())
        }
    }

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

    fn occupied_answer_with_args(pid: u32, command: &str, args: &str) -> FakeAnswer {
        FakeAnswer::stdout(marked(&format!(
            r#"{{"occupied":true,"pid":{pid},"command":"{command}","args":"{args}"}}"#
        )))
    }

    fn npm_dsh_args() -> &'static str {
        "/home/demo/.local/share/dsh-launcher/versions/0.2.1-rc.2/node_modules/@deepseek-ai/dsh/lib/bin.js --profile web --port 3099 --no-open"
    }

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
        let script = match &fake.calls()[0] {
            FakeCall::Run(script) => script.clone(),
            other => panic!("{other:?}"),
        };
        let check = script.find("dsh_current\" != \"$dsh_started\"").unwrap();
        let signal = script.find("kill -TERM").unwrap();
        assert!(check < signal, "核对要在发信号之前：{script}");
    }

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

    #[tokio::test]
    async fn a_pinned_port_with_a_dsh_like_listener_is_adopted() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::failed(2, "远端没有这个文件：running.json"))
            .answering(occupied_answer_with_args(4242, "node", npm_dsh_args()))
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

    #[tokio::test]
    async fn a_restarted_remote_instance_is_found_on_its_pinned_port() {
        let printed = format!("dsh web: {READY_URL}\n");
        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(marked(
                r#"{"occupied":false,"pid":null,"command":"","args":""}"#,
            )))
            .answering(occupied_answer_with_args(4343, "node", npm_dsh_args()))
            .answering(FakeAnswer::stdout(marked(r#"{"started_at":117384999}"#)))
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
            ))));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, Some(3080));
        let mut gone = remote_run();
        gone.host_id = host.id.clone();
        gone.pid = 4242;
        gone.remote_url = Some(READY_URL.to_string());

        let found = await_remote_successor(&state, &inst, &host, &gone)
            .await
            .expect("那台机器要回答");

        let (pid, started_at, url) = found.expect("重生的进程要被找到");
        assert_eq!(pid, 4343, "认领的是新 pid，不是已经消失的那个");
        assert_eq!(started_at, Some(117384999));
        assert_eq!(url.as_deref(), Some(READY_URL), "新进程打印的地址要带回来");
    }

    #[tokio::test]
    async fn a_successor_probe_that_cannot_ask_the_machine_says_unknown() {
        let fake = FakeTransport::new().answering(FakeAnswer::fail(
            crate::ssh::HostErrorKind::Unreachable,
            "ssh 连不上",
        ));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, Some(3080));
        let mut gone = remote_run();
        gone.host_id = host.id.clone();
        gone.pid = 4242;

        let result = await_remote_successor(&state, &inst, &host, &gone).await;

        assert!(
            result.is_err(),
            "问不到那台机器时不能把「没有后继」当成结论：{result:?}"
        );
    }

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

    #[tokio::test]
    async fn a_random_port_is_adopted_when_the_recorded_token_is_accepted_there() {
        let fake = FakeTransport::new()
            .answering(record_answer(Some(READY_URL)))
            .answering(occupied_answer_with_args(4242, "node", npm_dsh_args()))
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

    #[tokio::test]
    async fn a_random_port_whose_token_is_refused_is_not_adopted() {
        let fake = FakeTransport::new()
            .answering(record_answer(Some(READY_URL)))
            .answering(occupied_answer_with_args(4242, "node", npm_dsh_args()))
            .answering(FakeAnswer::stdout(marked(
                r#"{"status":401,"cookie":null}"#,
            )));
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let (state, inst, host) = remote_case(&fake, &host_id, None);

        let verdict = check_remote_instance(&state, &inst, &host).await;

        assert_eq!(verdict, RemoteVerdict::Stopped);
    }

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

    #[tokio::test]
    async fn instances_of_an_unreachable_host_read_as_unreachable() {
        let host_id = format!("host-{}", uuid::Uuid::new_v4());
        let state = remote_app_state(
            FakeTransport::new(),
            remote_host(&host_id),
            remote_instance(&host_id, Some(3080)),
        );

        finish_remote_scan(&state, &host_id);
        state.set_host_status(&host_id, HostStatus::unreachable("ssh 连不上"));
        let statuses = list_statuses(&state).await;
        assert_eq!(statuses.len(), 1, "{statuses:?}");
        assert_eq!(statuses[0].state, InstanceState::Unreachable);

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

        state.set_host_status(&host_id, HostStatus::reachable());
        let statuses = list_statuses(&state).await;
        assert_eq!(statuses[0].state, InstanceState::Running, "{statuses:?}");
        assert_eq!(statuses[0].url.as_deref(), Some(READY_URL));
    }

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

    #[test]
    fn manageable_url_requires_a_token() {
        let spawned = RunningEntry {
            id: "i1".to_string(),
            url: Some("http://127.0.0.1:1/?token=a".to_string()),
            adopted: false,
        };
        assert!(manageable_url(&spawned).is_some());

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

        let other = running_for_profile(&state, "h2", "web", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(other.url.as_deref(), Some("http://127.0.0.1:3/?token=c"));

        assert!(running_for_profile(&state, "h1", "demo", None)
            .await
            .unwrap()
            .is_none());
        assert!(running_for_profile(&state, "nope", "web", None)
            .await
            .is_err());
    }

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

        let first = running_for_profile(&state, "h1", "web", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.id, "i1");

        let preferred = running_for_profile(&state, "h1", "web", Some("i2"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preferred.id, "i2");
        assert_eq!(
            preferred.url.as_deref(),
            Some("http://127.0.0.1:2/?token=b")
        );

        for ignored in ["nope", "i9"] {
            let hit = running_for_profile(&state, "h1", "web", Some(ignored))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(hit.id, "i1", "{ignored} must not change the answer");
        }
    }

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
