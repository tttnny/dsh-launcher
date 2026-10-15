use crate::config::{
    new_id, DshHome, DshInstance, Host, HostStatus, LauncherSettings, RemoteVersion, SettingsPatch,
};
use crate::{process, AppState};
use std::collections::BTreeMap;
use tauri::{AppHandle, State};

/// The one path rule behind ADR-0002, defined next to the record model.
pub(crate) use crate::config::dedicated_home_path;

// ---------------------------------------------------------------------------
// Hosts
// ---------------------------------------------------------------------------

/// A host record plus the status last observed for it — one object per host, so
/// the settings page can render a row straight from it. `status` is runtime
/// (never in `config.json`), and `None` means 「还没测过」, which is not the same
/// as 「不可达」.
#[derive(Clone, Debug, serde::Serialize)]
pub struct HostView {
    #[serde(flatten)]
    pub host: Host,
    pub status: Option<HostStatus>,
}

/// The editable half of a host record: what the settings page's dialog holds.
/// Its fields keep the record's own spelling, so the page can hand back the
/// fields of the row it is editing.
///
/// `id` is deliberately absent — a host's id is the launcher's, and every record
/// that belongs to that host is filed under it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct HostInput {
    pub display_name: String,
    #[serde(default)]
    pub ssh_target: String,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
    #[serde(default)]
    pub data_dir: Option<String>,
}

/// The machines the launcher manages, each with the status last observed.
#[tauri::command]
pub fn list_hosts(state: State<'_, AppState>) -> Result<Vec<HostView>, String> {
    list_hosts_with_status(&state)
}

pub(crate) fn list_hosts_with_status(state: &AppState) -> Result<Vec<HostView>, String> {
    let hosts = state.config.lock().unwrap().hosts.clone();
    Ok(hosts
        .into_iter()
        .map(|host| HostView {
            status: state.host_status(&host.id),
            host,
        })
        .collect())
}

/// Adds a remote host. Nothing is dialed here: a machine that is switched off
/// right now can still be saved, and 「测试连接」 is what finds out whether it is
/// there (ADR-0006).
#[tauri::command]
pub fn create_host(state: State<'_, AppState>, input: HostInput) -> Result<Host, String> {
    create_host_record(&state, input)
}

pub(crate) fn create_host_record(state: &AppState, input: HostInput) -> Result<Host, String> {
    let host = Host {
        id: new_id("host"),
        display_name: checked_name(&input.display_name)?,
        ssh_target: checked_target(&input.ssh_target)?,
        registry: cleaned(input.registry),
        extra_env: input.extra_env,
        data_dir: cleaned_path(input.data_dir),
        builtin: false,
        // A host starts with nothing probed and nothing chosen: the environment
        // page is what fills both in.
        toolchain: Default::default(),
        toolchain_overrides: Default::default(),
    };
    let mut cfg = state.config.lock().unwrap();
    if let Some(existing) = cfg.hosts.iter().find(|h| h.ssh_target == host.ssh_target) {
        return Err(format!(
            "已经有一台主机用这个 ssh 目标：{}",
            existing.display_name
        ));
    }
    cfg.hosts.push(host.clone());
    crate::config::save_config(&state.config_path, &cfg)?;
    Ok(host)
}

/// Rewrites one host's editable fields. The record keeps its id and its
/// `builtin` flag: the form must not be able to turn 本机 into a remote machine,
/// or the other way round.
#[tauri::command]
pub fn update_host(
    state: State<'_, AppState>,
    id: String,
    input: HostInput,
) -> Result<Host, String> {
    update_host_record(&state, &id, input)
}

pub(crate) fn update_host_record(
    state: &AppState,
    id: &str,
    input: HostInput,
) -> Result<Host, String> {
    let name = checked_name(&input.display_name)?;
    let mut cfg = state.config.lock().unwrap();
    let pos = cfg
        .hosts
        .iter()
        .position(|h| h.id == id)
        .ok_or_else(|| format!("主机不存在: {id}"))?;
    let builtin = cfg.hosts[pos].builtin;
    // The probe cache and the user's choices are not part of this form: the
    // environment page owns them, and a settings save must not wipe them.
    let toolchain = cfg.hosts[pos].toolchain.clone();
    let toolchain_overrides = cfg.hosts[pos].toolchain_overrides.clone();
    let target = if builtin {
        // 本机 dials nothing; whatever a form sent for it is not a target.
        String::new()
    } else {
        checked_target(&input.ssh_target)?
    };
    cfg.hosts[pos] = Host {
        id: id.to_string(),
        display_name: name,
        ssh_target: target,
        registry: cleaned(input.registry),
        extra_env: input.extra_env,
        data_dir: cleaned_path(input.data_dir),
        builtin,
        toolchain,
        toolchain_overrides,
    };
    let updated = cfg.hosts[pos].clone();
    crate::config::save_config(&state.config_path, &cfg)?;
    Ok(updated)
}

/// Removes a host and the records filed under it — **only** the launcher's own
/// records. Nothing on that machine is touched, and the instances that were
/// running there keep running (ADR-0006, ADR-0007). What stays behind on that
/// machine — its data directory, versions, DSH_HOMEs and running record — is
/// what the page's confirmation names, with a command to go and look.
pub(crate) fn delete_host_record(state: &AppState, id: &str) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    let pos = cfg
        .hosts
        .iter()
        .position(|h| h.id == id)
        .ok_or_else(|| format!("主机不存在: {id}"))?;
    if cfg.hosts[pos].builtin {
        return Err("本机是内置的，不能删除".to_string());
    }
    cfg.hosts.remove(pos);

    let dropped: Vec<String> = cfg
        .instances
        .iter()
        .filter(|i| i.host_id == id)
        .map(|i| i.id.clone())
        .collect();
    cfg.instances.retain(|i| i.host_id != id);
    cfg.homes.retain(|h| h.host_id != id);
    cfg.versions.retain(|v| v.host_id != id);
    if cfg
        .settings
        .last_instance_id
        .as_ref()
        .is_some_and(|last| dropped.contains(last))
    {
        cfg.settings.last_instance_id = None;
    }
    crate::config::save_config(&state.config_path, &cfg)?;
    drop(cfg);
    state.forget_host_status(id);
    Ok(())
}

#[tauri::command]
pub fn delete_host(state: State<'_, AppState>, id: String) -> Result<(), String> {
    delete_host_record(&state, &id)
}

/// The connection test: what that machine is, where its home and the launcher's
/// data directory are, and what Node and pnpm it resolves to.
///
/// A machine that cannot be reached comes back as an `Ok` result saying so (the
/// record stays, and the page says 「不可达」); only a broken request — an unknown
/// host id, the built-in host — is an error.
#[tauri::command]
pub async fn test_host_connection(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::remote::ConnectionTest, String> {
    test_host_connection_for(&state, &id).await
}

pub(crate) async fn test_host_connection_for(
    state: &AppState,
    id: &str,
) -> Result<crate::remote::ConnectionTest, String> {
    let host = {
        let cfg = state.config.lock().unwrap();
        cfg.hosts
            .iter()
            .find(|h| h.id == id)
            .cloned()
            .ok_or_else(|| format!("主机不存在: {id}"))?
    };
    if host.builtin {
        return Err("本机就在这台 Mac 上，不需要测试连接".to_string());
    }
    state.set_host_status(id, HostStatus::connecting());

    let target = crate::ssh::RemoteHost::of(&host, &state.data_dir).map_err(|e| e.message)?;
    let configured = host
        .data_dir
        .as_ref()
        .map(|dir| dir.to_string_lossy().to_string());
    let test =
        crate::remote::connection_test(&*state.host_transport, &target, configured.as_deref())
            .await;

    state.set_host_status(
        id,
        if test.ok {
            HostStatus::reachable()
        } else {
            HostStatus::unreachable(test.message.clone().unwrap_or_default())
        },
    );
    // A machine that answered also told us where its data directory is. Remember
    // it, so later work on that host does not have to ask again; a value the user
    // configured always wins.
    if test.ok && host.data_dir.is_none() {
        if let Some(dir) = test.data_dir.clone() {
            remember_data_dir(state, id, &dir);
        }
    }
    Ok(test)
}

fn remember_data_dir(state: &AppState, id: &str, dir: &str) {
    let mut cfg = state.config.lock().unwrap();
    let Some(host) = cfg.hosts.iter_mut().find(|h| h.id == id) else {
        return;
    };
    if host.data_dir.is_some() {
        return;
    }
    host.data_dir = Some(std::path::PathBuf::from(dir));
    if let Err(e) = crate::config::save_config(&state.config_path, &cfg) {
        crate::log_warn!("记住主机 {id} 的远端数据目录失败：{e}");
    }
}

/// The aliases of the user's own `~/.ssh/config`, for the add/edit dialog's
/// picker. A machine with no config simply has none.
#[tauri::command]
pub fn list_ssh_aliases() -> Result<Vec<crate::ssh::SshAlias>, String> {
    Ok(crate::ssh::ssh_aliases())
}

fn checked_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("主机的显示名不能为空".to_string());
    }
    Ok(name.to_string())
}

fn checked_target(raw: &str) -> Result<String, String> {
    let target = raw.trim();
    if target.is_empty() {
        return Err("请填写 ssh 目标：~/.ssh/config 里的别名，或 user@host 这样的字符串".to_string());
    }
    Ok(target.to_string())
}

/// An empty field in the dialog means 「没配」, not an empty string.
fn cleaned(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn cleaned_path(value: Option<String>) -> Option<std::path::PathBuf> {
    cleaned(value).map(std::path::PathBuf::from)
}

// ---------------------------------------------------------------------------
// DSH_HOME
// ---------------------------------------------------------------------------

/// One host's DSH_HOME records; `host_id` absent means 本机.
///
/// A remote host's own `~/.dsh` is asked of **that machine** here, with one
/// ssh — it is the only one that can say whether it has one, and 本机's
/// equivalent runs at load time ([`crate::config::ensure_user_dsh_home`]).
/// That question is the only part of this answer a machine is needed for: the
/// records themselves are the launcher's own (ADR-0006 §2), so a machine that
/// does not answer leaves the list as it stands instead of emptying it.
/// 「装了哪些版本」 is where the Library reports that a host did not answer.
#[tauri::command]
pub async fn list_homes(
    state: State<'_, AppState>,
    host_id: Option<String>,
) -> Result<Vec<DshHome>, String> {
    list_homes_for(&state, host_id.as_deref()).await
}

pub(crate) async fn list_homes_for(
    state: &AppState,
    host_id: Option<&str>,
) -> Result<Vec<DshHome>, String> {
    let (host, target) = crate::runtime::env_host(state, host_id)?;
    if let crate::runtime::EnvHost::Remote(target) = &target {
        // Best effort, and deliberately so: this asks a question the records do
        // not depend on, and an unreachable machine is not a reason to withhold
        // a list the launcher keeps itself.
        let _ = adopt_user_dsh_home(state, &host, target).await;
    }
    let cfg = state.config.lock().unwrap();
    Ok(cfg
        .homes
        .iter()
        .filter(|h| h.host_id == host.id)
        .cloned()
        .collect())
}

/// Adds that machine's `~/.dsh` as a record when it has one and the launcher
/// does not know it yet — the remote twin of
/// [`crate::config::ensure_user_dsh_home`], which expands `~` against *this*
/// Mac's home and therefore can only ever be about 本机. The path recorded is
/// the machine's own answer.
///
/// One ssh, and no record write when the machine has no `~/.dsh`: a machine
/// with nothing on it is a machine, not an error. Whether a machine that did
/// not answer is also not an error is the caller's call, and [`list_homes_for`]
/// says how it answers that.
async fn adopt_user_dsh_home(
    state: &AppState,
    host: &Host,
    target: &crate::ssh::RemoteHost,
) -> Result<(), String> {
    let answer = crate::remote::user_home(&*state.host_transport, target)
        .await
        .map_err(|e| e.to_string())?;
    if !answer.exists || answer.dsh_home.is_empty() {
        return Ok(());
    }
    let home = DshHome {
        id: crate::config::user_dsh_home_id(host),
        name: crate::config::USER_DSH_HOME_NAME.to_string(),
        path: std::path::PathBuf::from(&answer.dsh_home),
        host_id: host.id.clone(),
    };
    // The check and the write share one lock: two reads of the same host racing
    // each other must not leave two records for one directory.
    let mut cfg = state.config.lock().unwrap();
    if cfg
        .homes
        .iter()
        .any(|h| h.host_id == host.id && host.paths_equal(&h.path, &home.path))
    {
        return Ok(());
    }
    cfg.homes.push(home);
    crate::config::save_config(&state.config_path, &cfg)
}

#[tauri::command]
pub async fn create_home(
    state: State<'_, AppState>,
    name: String,
    path: String,
    host_id: Option<String>,
) -> Result<DshHome, String> {
    create_home_record_in(&state, &name, &path, host_id.as_deref()).await
}

/// Shared helper: validates + creates a DSH_HOME record on one host, and
/// answers the path that host settled on.
///
/// The path is a path **on that machine**, so that machine is what resolves it.
/// 本机 goes through [`crate::config::normalize_dir_path`] and
/// `std::fs::create_dir_all`; a remote host asks that server instead
/// ([`crate::remote::ensure_home_path`]) — a local `create_dir_all` aimed at a
/// remote spelling would leave a stray directory on this Mac, and `~` would
/// expand to *this* machine's home.
///
/// If a HOME with the same path already exists *on the same host*, the existing
/// record is returned instead of creating a duplicate (prevents duplicate
/// same-name HOMEs when a dedicated HOME is requested repeatedly). Path equality
/// is the host's own ([`crate::config::Host::paths_equal`]): 本机 folds case,
/// a remote Linux host does not, where two spellings are two directories.
///
/// `host_id` absent means 本机.
pub(crate) async fn create_home_record_in(
    state: &AppState,
    name: &str,
    path: &str,
    host_id: Option<&str>,
) -> Result<DshHome, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名称不能为空".to_string());
    }

    let (host, target) = crate::runtime::env_host(state, host_id)?;
    let path = match target {
        crate::runtime::EnvHost::Local => {
            let path = crate::config::normalize_dir_path(path)?;
            std::fs::create_dir_all(&path).map_err(|e| format!("创建目录失败: {e}"))?;
            path
        }
        crate::runtime::EnvHost::Remote(target) => {
            let resolved = crate::remote::ensure_home_path(&*state.host_transport, &target, path)
                .await
                .map_err(|e| e.to_string())?;
            std::path::PathBuf::from(resolved)
        }
    };

    // Reuse an existing HOME with the same path on the same host.
    let existing = {
        let cfg = state.config.lock().unwrap();
        cfg.homes
            .iter()
            .find(|h| h.host_id == host.id && host.paths_equal(&h.path, &path))
            .cloned()
    };
    if let Some(existing) = existing {
        return Ok(existing);
    }

    let home = DshHome {
        id: new_id("h"),
        name: name.to_string(),
        path,
        host_id: host.id,
    };
    let mut cfg = state.config.lock().unwrap();
    cfg.homes.push(home.clone());
    crate::config::save_config(&state.config_path, &cfg)?;
    Ok(home)
}

#[tauri::command]
pub fn default_dedicated_home_path(
    state: State<'_, AppState>,
    name: String,
    host_id: Option<String>,
) -> Result<String, String> {
    let cfg = state.config.lock().unwrap();
    let host = crate::config::resolve_host(&cfg, host_id.as_deref())?;
    Ok(host
        .dedicated_home_path(&state.data_dir, &name)?
        .to_string_lossy()
        .to_string())
}

/// Removes one version's dedicated DSH_HOME, judged by path alone. A path that
/// no longer exists (already cleaned by hand) is not an error. Removal is
/// best-effort: a failure is logged but never aborts the version deletion —
/// the version record and its instances are already gone by then.
pub(crate) fn remove_dedicated_home(
    data_dir: &std::path::Path,
    version: &str,
) -> Result<(), String> {
    let home = dedicated_home_path(data_dir, version);
    match std::fs::remove_dir_all(&home) {
        Ok(()) => {
            crate::log_info!("已删除版本 {} 的专属 DSH_HOME {}", version, home.display());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除专属 DSH_HOME 失败: {e}")),
    }
}

/// Drops the version's dedicated DSH_HOME record, by the same path rule its
/// directory is removed with (ADR-0002). A record left behind outlives what it
/// names: the DSH_HOME list keeps offering the deleted version's HOME, and
/// opening it puts the directory back, which makes the startup orphan sweep
/// read it as a real HOME from then on.
///
/// `data_dir` is `host`'s data directory, and only `host`'s records are
/// judged: the same path on another machine is a different HOME.
pub(crate) fn remove_dedicated_home_record(
    cfg: &mut crate::config::Config,
    host: &Host,
    data_dir: &std::path::Path,
    version: &str,
) {
    let home = dedicated_home_path(data_dir, version);
    cfg.homes
        .retain(|h| !(h.host_id == host.id && host.paths_equal(&h.path, &home)));
}

#[tauri::command]
pub fn remove_home(
    state: State<'_, AppState>,
    id: String,
    host_id: Option<String>,
) -> Result<(), String> {
    remove_home_record(&state, &id, host_id.as_deref())
}

/// Forgets one DSH_HOME record. **A record is all it is**: nothing is asked of
/// the machine that owns the directory, and nothing there is removed — the same
/// rule as 本机 (ADR-0006), and the reason a remote host needs no transport
/// here.
pub(crate) fn remove_home_record(
    state: &AppState,
    id: &str,
    host_id: Option<&str>,
) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    let host = crate::config::resolve_host(&cfg, host_id)?.clone();
    if cfg.instances.iter().any(|i| i.home_id == id) {
        return Err("该 DSH_HOME 仍被实例引用，无法删除".to_string());
    }
    cfg.homes
        .retain(|h| !(h.id == id && h.host_id == host.id));
    save_state(state, &cfg)
}

// ---------------------------------------------------------------------------
// DSH versions
// ---------------------------------------------------------------------------

/// A version as the frontend sees it: the stored record plus its 专属
/// DSH_HOME, derived here (where the host's data directory is known) so the
/// delete dialog can name the exact path it removes and no second
/// implementation of the path rule exists on the JS side.
#[derive(Clone, Debug, serde::Serialize)]
pub struct DshVersionView {
    pub id: String,
    pub version: String,
    pub dir: std::path::PathBuf,
    pub dedicated_home: String,
    pub host_id: String,
    /// Whether the version's bin is there and not empty. A directory an install
    /// left half-finished is a record the user can see and delete, but not a
    /// version anyone can start — and only the machine that owns the directory
    /// can say which it is ([`crate::launch::version_bin`] is the layout).
    pub bin_ready: bool,
}

#[tauri::command]
pub async fn list_versions(
    state: State<'_, AppState>,
    host_id: Option<String>,
) -> Result<Vec<DshVersionView>, String> {
    list_versions_for(&state, host_id.as_deref()).await
}

/// One host's installed versions.
///
/// The records are the launcher's: they say which versions were installed on
/// that host. Everything the page shows beyond them is what only the machine
/// knows, and one ssh answers for all of them at once — so a host that cannot
/// be reached fails the whole call rather than answering from records alone
/// (「装了哪些」 would then be a claim the launcher cannot back up).
pub(crate) async fn list_versions_for(
    state: &AppState,
    host_id: Option<&str>,
) -> Result<Vec<DshVersionView>, String> {
    let (host, data_dir, records) = {
        let cfg = state.config.lock().unwrap();
        let host = crate::config::resolve_host(&cfg, host_id)?.clone();
        let data_dir = host.data_dir(&state.data_dir)?;
        let records: Vec<crate::config::DshVersion> = cfg
            .versions
            .iter()
            .filter(|v| v.host_id == host.id)
            .cloned()
            .collect();
        (host, data_dir, records)
    };

    let mut listed: Vec<DshVersionView> = records
        .iter()
        .map(|v| DshVersionView {
            id: v.id.clone(),
            version: v.version.clone(),
            dir: v.dir.clone(),
            dedicated_home: dedicated_home_path(&data_dir, &v.version)
                .to_string_lossy()
                .to_string(),
            host_id: v.host_id.clone(),
            bin_ready: false,
        })
        .collect();

    if host.builtin {
        for view in &mut listed {
            view.bin_ready = crate::launch::version_bin_ready(&view.dir);
        }
        return Ok(listed);
    }

    // A remote host states its own data directory; without one there is no
    // versions directory to ask about, and guessing is not an option.
    let target = crate::ssh::RemoteHost::of(&host, &state.data_dir).map_err(|e| e.message)?;
    let versions_dir = data_dir.join("versions");
    let on_machine = crate::remote::installed_versions(
        &*state.host_transport,
        &target,
        &versions_dir.to_string_lossy(),
    )
    .await
    .map_err(|e| e.to_string())?;

    for view in &mut listed {
        if let Some(found) = on_machine.iter().find(|f| f.name == view.version) {
            // The machine's own answer about where it is and whether it is
            // usable wins over the record's copy of the path.
            view.dir = std::path::PathBuf::from(&found.dir);
            view.bin_ready = found.bin_ready;
        }
    }
    Ok(listed)
}

/// Where one host's installable-version list is fetched.
///
/// 本机 asks its own npm. A remote host with a registry of its own asks **that**
/// machine's npm: its registry may be a mirror only that machine can reach, and
/// a list fetched here could offer versions that cannot be installed there. A
/// remote host with no registry is listed from the very registry the launcher
/// already queries, so it is fetched here — no ssh, and no second way to fail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionsSource {
    Local { registry: Option<String> },
    Remote { registry: String },
}

/// Which npm answers for one host, and with which registry.
pub fn versions_source(host: &Host) -> VersionsSource {
    let registry = host
        .registry
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    if host.builtin {
        return VersionsSource::Local { registry };
    }
    match registry {
        Some(registry) => VersionsSource::Remote { registry },
        None => VersionsSource::Local { registry: None },
    }
}

/// Turns npm's two `--json` answers into the list the page shows. npm >= 9
/// returns `time` as `[{ ... }]` (an array wrapping one object); older npm
/// returned the object directly.
pub(crate) fn versions_from_npm(
    versions_json: &str,
    time_json: &str,
) -> Result<Vec<RemoteVersion>, String> {
    let versions: Vec<String> =
        serde_json::from_str(versions_json).map_err(|e| format!("解析版本列表失败: {e}"))?;
    let time_map: BTreeMap<String, serde_json::Value> =
        match serde_json::from_str::<BTreeMap<String, serde_json::Value>>(time_json) {
            Ok(map) => map,
            Err(_) => {
                let arr: Vec<BTreeMap<String, serde_json::Value>> =
                    serde_json::from_str(time_json).map_err(|e| format!("解析发布时间失败: {e}"))?;
                arr.into_iter().next().unwrap_or_default()
            }
        };

    let mut out = Vec::with_capacity(versions.len());
    for v in versions {
        let released_at = time_map
            .get(&v)
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        out.push(RemoteVersion {
            version: v,
            released_at,
            source: None,
        });
    }
    Ok(out)
}

/// Queries the npm registry for available @deepseek-ai/dsh versions with
/// their publish dates, then merges in GitHub-only `dsh-v*` release tags
/// (alpha builds are tagged on GitHub but not published to npm) marked with
/// `source: "github"`.
///
/// `host_id` absent means 本机. A remote host with a registry of its own is
/// queried on that machine (see [`versions_source`]).
#[tauri::command]
pub async fn fetch_available_versions(
    state: State<'_, AppState>,
    host_id: Option<String>,
) -> Result<Vec<RemoteVersion>, String> {
    let host = {
        let cfg = state.config.lock().unwrap();
        crate::config::resolve_host(&cfg, host_id.as_deref())?.clone()
    };
    let mut out = match versions_source(&host) {
        VersionsSource::Local { registry } => {
            let versions_json = run_npm_view("@deepseek-ai/dsh", "versions", registry.as_deref()).await?;
            let time_json = run_npm_view("@deepseek-ai/dsh", "time", registry.as_deref()).await?;
            versions_from_npm(&versions_json, &time_json)?
        }
        VersionsSource::Remote { registry } => {
            let target = crate::ssh::RemoteHost::of(&host, &state.data_dir).map_err(|e| e.message)?;
            let listings = crate::remote::available_versions(
                &*state.host_transport,
                &target,
                &registry,
            )
            .await
            .map_err(|e| e.to_string())?;
            versions_from_npm(&listings.versions, &listings.time)?
        }
    };

    // GitHub-only tags (e.g. dsh-v0.1.2-alpha.1). A release listing failure
    // never aborts the npm listing.
    match fetch_github_tag_versions().await {
        Ok(git_versions) => {
            for gv in git_versions {
                if out.iter().any(|r| r.version == gv.version) {
                    continue;
                }
                out.push(gv);
            }
        }
        Err(e) => crate::log_warn!("获取 GitHub dsh-v* 标签失败，忽略: {e}"),
    }
    Ok(out)
}

/// Upstream repo whose `dsh-v<version>` tags carry releases.
pub(crate) const DSH_REPO: &str = "deepseek-ai/deepseek-harness";

/// Versions tagged on GitHub as `dsh-v*` releases (whether or not they were
/// later published to npm — dedup happens at the caller).
async fn fetch_github_tag_versions() -> Result<Vec<RemoteVersion>, String> {
    let url = crate::plugins::github_api_url(&format!("/repos/{DSH_REPO}/releases?per_page=100"));
    let doc = crate::plugins::fetch_json(&url, 8 * 1024 * 1024).await?;
    let mut out = Vec::new();
    let Some(arr) = doc.as_array() else {
        return Ok(out);
    };
    for rel in arr {
        if rel.get("draft").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let Some(tag) = rel.get("tag_name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(version) = tag.strip_prefix("dsh-v") else {
            continue;
        };
        // A Git tag may legally contain `/`, `..`, and other characters that
        // are unsafe as the version-directory path segment the installer
        // joins it into — only accept well-formed version strings.
        if !crate::tasks::valid_version_string(version) {
            crate::log_warn!("忽略格式异常的 GitHub 标签: {tag}");
            continue;
        }
        out.push(RemoteVersion {
            version: version.to_string(),
            released_at: rel
                .get("published_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            source: Some("github".to_string()),
        });
    }
    Ok(out)
}

async fn run_npm_view(
    pkg: &str,
    field: &str,
    registry: Option<&str>,
) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(process::npm());
    // Keep the listing consistent with the installer's npm probe: both honor
    // the DSH_NPM_REGISTRY mirror, so a mirror user doesn't see official-
    // registry data in the list but mirror data at install time (or vice
    // versa when the mirror lags). A registry named on the host record wins:
    // that is the registry this machine's versions come from.
    crate::proxy::apply_to_command(&mut cmd);
    cmd.args(["view", pkg, field, "--json"]);
    if let Some(registry) = registry.map(str::to_string).or_else(crate::toolchain::registry_mirror) {
        cmd.args(["--registry", &registry]);
    }
    let output = crate::process::hide_console(&mut cmd)
        .output()
        .await
        .map_err(|e| format!("npm 执行失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "npm view 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub async fn remove_version(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    host_id: Option<String>,
) -> Result<(), String> {
    let (version, instance_ids, host, data_dir) = {
        let cfg = state.config.lock().unwrap();
        let host = crate::config::resolve_host(&cfg, host_id.as_deref())?.clone();
        let data_dir = host.data_dir(&state.data_dir)?;
        let Some(version) = cfg
            .versions
            .iter()
            .find(|v| v.id == id && v.host_id == host.id)
            .cloned()
        else {
            return Err("版本不存在".to_string());
        };
        let inst_ids: Vec<String> = cfg
            .instances
            .iter()
            .filter(|i| i.version_id == id)
            .map(|i| i.id.clone())
            .collect();
        (version, inst_ids, host, data_dir)
    };

    // Stop and cascade delete associated instances. A stop that did not take is
    // **not** swallowed: deleting the record of an instance whose process is
    // still running on that machine would leave the launcher unable to name what
    // it left behind (ADR-0007 — the launcher forgets, the server keeps running).
    for inst_id in &instance_ids {
        if state.running.lock().await.contains_key(inst_id) {
            process::stop_instance_process(&app, &state, inst_id)
                .await
                .map_err(|e| format!("实例 {inst_id} 没能停下来，版本没有删：{e}"))?;
        }
    }

    {
        let mut cfg = state.config.lock().unwrap();
        cfg.instances.retain(|i| i.version_id != id);
        if let Some(last_id) = &cfg.settings.last_instance_id {
            if !cfg.instances.iter().any(|i| &i.id == last_id) {
                cfg.settings.last_instance_id = None;
            }
        }
        cfg.versions.retain(|v| v.id != id);
        remove_dedicated_home_record(&mut cfg, &host, &data_dir, &version.version);
        save_state(&state, &cfg)?;
    }

    // Everything below removes directories the *host* owns, so it is done on
    // that host: this Mac's filesystem for 本机, and the transport for a remote
    // one — a local `remove_dir_all` aimed at a remote path would delete
    // whatever this Mac happens to have at the same spelling, and the record
    // that named what it destroyed is already gone by now.
    if !host.builtin {
        match host.dedicated_home_path(&state.data_dir, &version.version) {
            Ok(home) => {
                remove_remote_version_dirs(&state, &host, &data_dir, &[&version.dir, &home]).await
            }
            Err(e) => crate::log_warn!("无法确定版本 {} 的专属 DSH_HOME：{e}", version.version),
        }
        return Ok(());
    }

    // Best-effort removal of the install directory.
    let _ = std::fs::remove_dir_all(&version.dir);

    // The version's 专属 DSH_HOME goes with it, judged by path alone
    // (ADR-0002): `<该主机的数据目录>/homes/<版本名>` is the one path the
    // launcher can prove it created, so `~/.dsh` and any user-specified HOME —
    // which are also DSH_HOMEs, possibly pointed at by an instance — are never
    // touched. This runs after the state save: the state is the source of
    // truth, and a HOME left behind by a save-then-fail is recoverable, unlike
    // the reverse. Best-effort, so a foreign file inside the HOME cannot block
    // the delete.
    if let Err(e) = remove_dedicated_home(&data_dir, &version.version) {
        crate::log_warn!("删除版本 {} 的专属 DSH_HOME 失败，忽略: {e}", version.version);
    }
    Ok(())
}

/// The same two removals on a remote host: `<该主机的数据目录>/versions/<版本名>`
/// and `<该主机的数据目录>/homes/<版本名>`, each over the transport, and each
/// refused unless it is inside that host's own data directory — the one boundary
/// that keeps a remote delete from reaching anything else on that machine
/// (ADR-0006). A directory that is not there is already what the caller wanted.
///
/// Best effort, like the local half: the records are gone by now, and a failure
/// to free disk on the server is worth a log line, not a failed command that
/// would suggest the version is still there.
async fn remove_remote_version_dirs(
    state: &AppState,
    host: &Host,
    data_dir: &std::path::Path,
    paths: &[&std::path::Path],
) {
    let target = match crate::ssh::RemoteHost::of(host, &state.data_dir) {
        Ok(target) => target,
        Err(e) => {
            crate::log_warn!("删除远端版本目录失败：{}", e.message);
            return;
        }
    };
    for path in paths {
        if !crate::tasks::inside(data_dir, path) {
            crate::log_warn!(
                "拒绝删除远端数据目录之外的路径 {}（{}）",
                path.display(),
                data_dir.display()
            );
            continue;
        }
        if let Err(e) =
            crate::remote::remove_dir(&*state.host_transport, &target, &path.to_string_lossy())
                .await
        {
            crate::log_warn!("删除远端目录 {} 失败，忽略: {e}", path.display());
        }
    }
}

// ---------------------------------------------------------------------------
// Instances
// ---------------------------------------------------------------------------

/// Every instance of one host — or, when asked explicitly, of every host.
///
/// `all_hosts` is the board's 全部 view (ADR-0006 §3): the launcher has one
/// current host, and 「所有机器上的实例都列出来」 is a request only the caller can
/// make. Silence means 本机, exactly as it does everywhere else.
#[tauri::command]
pub fn list_instances(
    state: State<'_, AppState>,
    host_id: Option<String>,
    all_hosts: Option<bool>,
) -> Result<Vec<DshInstance>, String> {
    list_instances_for(&state, host_id.as_deref(), all_hosts.unwrap_or(false))
}

pub(crate) fn list_instances_for(
    state: &AppState,
    host_id: Option<&str>,
    all_hosts: bool,
) -> Result<Vec<DshInstance>, String> {
    let cfg = state.config.lock().unwrap();
    if all_hosts {
        return Ok(cfg.instances.clone());
    }
    let host = crate::config::resolve_host(&cfg, host_id)?;
    Ok(cfg
        .instances
        .iter()
        .filter(|i| i.host_id == host.id)
        .cloned()
        .collect())
}

/// Creates a new instance on an already-installed version.
///
/// Installing a version materializes one 1:1 instance; this is how a second
/// instance of the same version is added (the README's "同一版本可建多个实例"),
/// and the only path that binds a version to a *different* HOME. Version and
/// HOME must be on the host the instance is created on: a binding that spans
/// two machines is not a binding.
///
/// `host_id` absent means 本机.
#[tauri::command]
pub fn create_instance(
    state: State<'_, AppState>,
    name: String,
    version_id: String,
    home_id: String,
    host_id: Option<String>,
) -> Result<DshInstance, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("实例名称不能为空".to_string());
    }
    let mut cfg = state.config.lock().unwrap();
    let host = crate::config::resolve_host(&cfg, host_id.as_deref())?.clone();
    // Names are unique **per host**: two machines may each have an instance
    // called 「web」, and the row, the tray and the plugin link all keep telling
    // them apart by host. A name only has to be unambiguous where it is used.
    if cfg
        .instances
        .iter()
        .any(|i| i.name == name && i.host_id == host.id)
    {
        return Err("同名实例已存在".to_string());
    }
    let Some(version) = cfg.versions.iter().find(|v| v.id == version_id) else {
        return Err("DSH 版本不存在".to_string());
    };
    if version.host_id != host.id {
        return Err("DSH 版本不属于该主机".to_string());
    }
    let Some(home) = cfg.homes.iter().find(|h| h.id == home_id) else {
        return Err("DSH_HOME 不存在".to_string());
    };
    if home.host_id != host.id {
        return Err("DSH_HOME 不属于该主机".to_string());
    }
    let inst = DshInstance {
        id: new_id("i"),
        name,
        version_id,
        home_id,
        host_id: host.id,
        env_overrides: Default::default(),
        default_profile: None,
        last_profile: None,
        icon: None,
        port: None,
    };
    cfg.instances.push(inst.clone());
    save_state(&state, &cfg)?;
    Ok(inst)
}

/// Deletes one instance record. The version, the DSH_HOME and every other
/// instance are untouched — an instance is a binding, not a directory. Only the
/// instance's own icon file (which the launcher itself wrote into the HOME's
/// `icons/` folder — on this Mac, or on the machine the instance runs on) is
/// removed, and that removal is best-effort.
#[tauri::command]
pub async fn delete_instance(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    host_id: Option<String>,
) -> Result<(), String> {
    // Stop first: a running process holds the DSH_HOME and would outlive its
    // own record, leaving an orphan nothing can stop from the UI. A stop that
    // did not take is reported rather than swallowed: the record is the only
    // thing that names the process, and deleting it first is what turns 「停止
    // 失败」 into 「启动器忘了，服务器上还在跑」 (ADR-0007).
    if state.running.lock().await.contains_key(&id) {
        process::stop_instance_process(&app, &state, &id)
            .await
            .map_err(|e| format!("实例没能停下来，记录没有删：{e}"))?;
    }

    // Resolve where the icon file is — which machine, and which HOME — before
    // saving: the record is the only thing that names the file.
    let icon_scope = {
        let mut cfg = state.config.lock().unwrap();
        let host = crate::config::resolve_host(&cfg, host_id.as_deref())?.clone();
        let Some(pos) = cfg
            .instances
            .iter()
            .position(|i| i.id == id && i.host_id == host.id)
        else {
            return Err("实例不存在".to_string());
        };
        let inst = cfg.instances.remove(pos);
        if cfg.settings.last_instance_id.as_deref() == Some(id.as_str()) {
            cfg.settings.last_instance_id = None;
        }
        let icon_scope = if inst.icon.as_deref() == Some("local") {
            cfg.homes
                .iter()
                .find(|h| h.id == inst.home_id)
                .map(|h| (host, h.path.clone()))
        } else {
            None
        };
        save_state(&state, &cfg.clone())?;
        icon_scope
    };
    if let Some((host, home)) = icon_scope {
        crate::icons::remove_icon_file(&state, &host, &home, &id).await;
    }
    Ok(())
}

#[tauri::command]
pub fn update_instance(
    state: State<'_, AppState>,
    input: DshInstance,
    host_id: Option<String>,
) -> Result<DshInstance, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err("实例名称不能为空".to_string());
    }
    let mut cfg = state.config.lock().unwrap();
    let host = crate::config::resolve_host(&cfg, host_id.as_deref())?.clone();
    if cfg
        .instances
        .iter()
        .any(|i| i.name == name && i.id != input.id && i.host_id == host.id)
    {
        return Err("同名实例已存在".to_string());
    }
    if !cfg.versions.iter().any(|v| v.id == input.version_id) {
        return Err("DSH 版本不存在".to_string());
    }
    if !cfg.homes.iter().any(|h| h.id == input.home_id) {
        return Err("DSH_HOME 不存在".to_string());
    }
    let mut updated = input;
    updated.name = name;
    let Some(pos) = cfg.instances.iter().position(|i| i.id == updated.id) else {
        return Err("实例不存在".to_string());
    };
    if cfg.instances[pos].host_id != host.id {
        return Err("实例不属于该主机".to_string());
    }
    // The icon is managed by set/clear_instance_icon, the port by
    // set_instance_port and the owning host by the host a record was created
    // on, not by this form payload; preserve whatever is currently stored (the
    // frontend spreads a possibly stale instance).
    updated.icon = cfg.instances[pos].icon.clone();
    updated.port = cfg.instances[pos].port;
    updated.host_id = cfg.instances[pos].host_id.clone();
    cfg.instances[pos] = updated.clone();
    save_state(&state, &cfg)?;
    Ok(updated)
}

/// Sets an instance's preferred web port (issue #21). Empty / out-of-range
/// input (0, negative, > 65535, non-integer) means "random" — stored as
/// `None`, so launch passes `--port 0`.
#[tauri::command(rename_all = "snake_case")]
pub fn set_instance_port(
    state: State<'_, AppState>,
    instance_id: String,
    port: Option<i64>,
) -> Result<crate::config::DshInstance, String> {
    let port = match port {
        Some(p) if (1..=65535).contains(&p) => Some(p as u16),
        _ => None,
    };
    let mut cfg = state.config.lock().unwrap();
    let Some(inst) = cfg.instances.iter_mut().find(|i| i.id == instance_id) else {
        return Err("实例不存在".to_string());
    };
    inst.port = port;
    let updated = inst.clone();
    save_state(&state, &cfg)?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

/// One DSH_HOME record plus the machine it lives on.
///
/// This is the single place that decides 「this Mac's filesystem or the other
/// machine's」 for the DSH_HOME and Profile surface (ADR-0006). The path is the
/// same string either way; what differs is who touches it. 本机 reads and writes
/// it directly, a remote host only through the transport, and the caller never
/// has to know which — a profile operation is one call on this type.
struct HomeOn {
    home: DshHome,
    /// `None` when the HOME is on this Mac.
    remote: Option<crate::ssh::RemoteHost>,
    transport: std::sync::Arc<dyn crate::ssh::HostTransport>,
}

impl HomeOn {
    /// The DSH_HOME record named, resolved to wherever it lives.
    fn of(state: &AppState, home_id: &str) -> Result<Self, String> {
        let home = {
            let cfg = state.config.lock().unwrap();
            cfg.homes
                .iter()
                .find(|h| h.id == home_id)
                .cloned()
                .ok_or_else(|| "DSH_HOME 不存在".to_string())?
        };
        let (_, location) = crate::runtime::env_host(state, Some(&home.host_id))?;
        let remote = match location {
            crate::runtime::EnvHost::Local => None,
            crate::runtime::EnvHost::Remote(target) => Some(target),
        };
        Ok(Self {
            home,
            remote,
            transport: state.host_transport.clone(),
        })
    }

    /// The profiles directory under this HOME.
    fn profiles_dir(&self) -> std::path::PathBuf {
        crate::profile::profiles_dir(&self.home.path)
    }

    /// `<profiles>/<name>` on that machine. The name has to be one path segment
    /// (see [`crate::profile::profile_dir`]), so no caller can join a path that
    /// escapes `profiles/` — on this Mac or over ssh.
    fn profile_dir(&self, name: &str) -> Result<std::path::PathBuf, String> {
        crate::profile::profile_dir(&self.home.path, name)
    }

    /// The profile directories under this HOME, read wherever it lives. A HOME
    /// with no `profiles/` yet is an empty list, not an error.
    async fn list_profiles(&self) -> Result<Vec<String>, String> {
        let Some(target) = self.remote.as_ref() else {
            return Ok(list_profile_dirs(&self.profiles_dir()));
        };
        crate::remote::list_profiles(
            &*self.transport,
            target,
            &as_text(&self.profiles_dir()),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Copies one profile to a new name inside the same HOME. Nothing is
    /// overwritten: an existing target is refused, with the same answer 本机
    /// gives.
    async fn copy_profile(&self, source: &str, name: &str) -> Result<(), String> {
        let from = self.profile_dir(source)?;
        let to = self.profile_dir(name)?;
        let Some(target) = self.remote.as_ref() else {
            if !from.is_dir() {
                return Err(format!("Profile「{source}」不存在"));
            }
            if to.exists() {
                return Err(format!("Profile「{name}」已存在"));
            }
            return copy_dir_recursive(&from, &to).map_err(|e| format!("复制 Profile 失败: {e}"));
        };
        crate::remote::copy_profile(&*self.transport, target, &as_text(&from), &as_text(&to))
            .await
            .map_err(|e| e.to_string())
    }

    /// Renames one profile inside the same HOME.
    async fn rename_profile(&self, old_name: &str, new_name: &str) -> Result<(), String> {
        let from = self.profile_dir(old_name)?;
        let to = self.profile_dir(new_name)?;
        let Some(target) = self.remote.as_ref() else {
            if !from.is_dir() {
                return Err(format!("Profile「{old_name}」不存在"));
            }
            if to.exists() {
                return Err(format!("Profile「{new_name}」已存在"));
            }
            return std::fs::rename(&from, &to).map_err(|e| format!("重命名 Profile 失败: {e}"));
        };
        crate::remote::rename_profile(&*self.transport, target, &as_text(&from), &as_text(&to))
            .await
            .map_err(|e| e.to_string())
    }

    /// Removes one profile inside the same HOME. The two reserved names are
    /// refused before either filesystem is touched: they belong to the template
    /// machinery, on both machines.
    async fn delete_profile(&self, name: &str) -> Result<(), String> {
        let target_dir = self.profile_dir(name)?;
        if name == "__temp__" || name == "node_modules" {
            return Err(format!("「{name}」为保留名称，不能删除"));
        }
        let Some(target) = self.remote.as_ref() else {
            if !target_dir.is_dir() {
                return Err(format!("Profile「{name}」不存在"));
            }
            return std::fs::remove_dir_all(&target_dir)
                .map_err(|e| format!("删除 Profile 失败: {e}"));
        };
        crate::remote::delete_profile(&*self.transport, target, &as_text(&target_dir))
            .await
            .map_err(|e| e.to_string())
    }
}

/// A path in the spelling a remote script can carry.
fn as_text(path: &std::path::Path) -> String {
    path.to_string_lossy().to_string()
}

/// The profile directories under one `profiles/`, in this Mac's own filesystem.
fn list_profile_dirs(profiles_dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(profiles_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip the template and non-profile entries.
            if name == "node_modules" || name == "__temp__" {
                continue;
            }
            if entry.path().is_dir() {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_profiles(
    state: State<'_, AppState>,
    home_id: String,
) -> Result<Vec<String>, String> {
    list_profiles_for(&state, &home_id).await
}

/// One DSH_HOME's profile directories, read where the HOME lives.
pub(crate) async fn list_profiles_for(
    state: &AppState,
    home_id: &str,
) -> Result<Vec<String>, String> {
    HomeOn::of(state, home_id)?.list_profiles().await
}

/// Creates a new profile by copying the `__temp__` template from dedicated homes.
/// Never creates or overrides `__temp__` in user default ~/.dsh/profiles (issue #5).
///
/// 本机 only. The template hunt walks the launcher's own data directory and the
/// copy is this machine's directory copy; on a remote host both would have to be
/// asked of that machine, and a DSH_HOME that was just created there has nothing
/// to copy from yet. The refusal names the way out, so the button that cannot
/// work here does not leave the user guessing.
#[tauri::command(rename_all = "snake_case")]
pub fn create_profile(
    state: State<'_, AppState>,
    home_id: String,
    name: String,
) -> Result<String, String> {
    let name = name.trim().to_string();
    crate::profile::validate_profile_name(&name)?;

    let (home_path, is_user_default) = {
        let cfg = state.config.lock().unwrap();
        let home = cfg
            .homes
            .iter()
            .find(|h| h.id == home_id)
            .cloned()
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        let host = crate::config::resolve_host(&cfg, Some(&home.host_id))?;
        if !host.builtin {
            return Err(format!(
                "还不能在「{}」上新建 Profile：请在那台机器的 <DSH_HOME>/profiles 里把一个 Profile 复制成新名字，再点「重新读取 Profile」",
                host.display_name
            ));
        }
        let is_user_default = home.id == crate::config::USER_DSH_HOME_ID
            || std::env::var("HOME")
                .map(|h| home.path == std::path::PathBuf::from(h).join(".dsh"))
                .unwrap_or(false);
        (home.path, is_user_default)
    };
    let profiles_dir = crate::profile::profiles_dir(&home_path);

    let target = profiles_dir.join(&name);
    if target.exists() {
        return Err(format!("Profile「{name}」已存在"));
    }

    // Locate __temp__ template strictly from dedicated homes:
    // Never create or overwrite __temp__ in ~/.dsh/profiles.
    let template_path = if !is_user_default && profiles_dir.join("__temp__").is_dir() {
        profiles_dir.join("__temp__")
    } else if !is_user_default && profiles_dir.join("web").is_dir() {
        let temp_dir = profiles_dir.join("__temp__");
        copy_dir_recursive(&profiles_dir.join("web"), &temp_dir)
            .map_err(|e| format!("初始化模板失败: {e}"))?;
        crate::profile::scrub_profile_port_pin(&temp_dir);
        temp_dir
    } else {
        // Look in launcher's dedicated homes (<data_dir>/homes/*/profiles/__temp__):
        let homes_root = state.data_dir.join("homes");
        let mut found = None;
        if let Ok(entries) = std::fs::read_dir(&homes_root) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("profiles").join("__temp__");
                if candidate.is_dir() {
                    found = Some(candidate);
                    break;
                }
            }
        }
        found.ok_or_else(|| "未找到可用的 Profile __temp__ 模板，请先安装至少一个 DSH 版本".to_string())?
    };

    if !profiles_dir.is_dir() {
        std::fs::create_dir_all(&profiles_dir).map_err(|e| format!("创建 profiles 目录失败: {e}"))?;
    }

    copy_dir_recursive(&template_path, &target).map_err(|e| format!("创建 Profile 失败: {e}"))?;
    Ok(name)
}

/// Copies an existing profile directory to a new name inside the same HOME.
/// The copy is only materialized after the new name is validated, mirroring
/// create_profile's copy-from-template behavior.
#[tauri::command(rename_all = "snake_case")]
pub async fn copy_profile(
    state: State<'_, AppState>,
    home_id: String,
    source: String,
    name: String,
) -> Result<String, String> {
    let name = name.trim().to_string();
    crate::profile::validate_profile_name(&name)?;
    copy_profile_for(&state, &home_id, &source, &name).await
}

/// Copies one profile to a new name inside the same HOME, wherever it lives.
pub(crate) async fn copy_profile_for(
    state: &AppState,
    home_id: &str,
    source: &str,
    name: &str,
) -> Result<String, String> {
    HomeOn::of(state, home_id)?.copy_profile(source, name).await?;
    Ok(name.to_string())
}

/// Renames a profile directory inside the given HOME.
#[tauri::command(rename_all = "snake_case")]
pub async fn rename_profile(
    state: State<'_, AppState>,
    home_id: String,
    old_name: String,
    new_name: String,
) -> Result<String, String> {
    let new_name = new_name.trim().to_string();
    crate::profile::validate_profile_name(&new_name)?;
    rename_profile_for(&state, &home_id, &old_name, &new_name).await
}

/// Renames one profile inside the same HOME, wherever it lives, and keeps the
/// instances that ran with it pointing at the new name.
pub(crate) async fn rename_profile_for(
    state: &AppState,
    home_id: &str,
    old_name: &str,
    new_name: &str,
) -> Result<String, String> {
    HomeOn::of(state, home_id)?
        .rename_profile(old_name, new_name)
        .await?;

    // Keep the instance's default/last profile references in sync.
    let mut cfg = state.config.lock().unwrap();
    for inst in cfg.instances.iter_mut() {
        if inst.home_id == home_id {
            if inst.default_profile.as_deref() == Some(old_name) {
                inst.default_profile = Some(new_name.to_string());
            }
            if inst.last_profile.as_deref() == Some(old_name) {
                inst.last_profile = Some(new_name.to_string());
            }
        }
    }
    save_state(state, &cfg)?;

    Ok(new_name.to_string())
}

/// Deletes a profile directory inside the given HOME. The default/last profile
/// references on instances using this HOME are cleared when they point at the
/// removed profile.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_profile(
    state: State<'_, AppState>,
    home_id: String,
    name: String,
) -> Result<(), String> {
    delete_profile_for(&state, &home_id, &name).await
}

/// Removes one profile inside the same HOME, wherever it lives, and clears the
/// default/last profile of the instances that pointed at it.
pub(crate) async fn delete_profile_for(
    state: &AppState,
    home_id: &str,
    name: &str,
) -> Result<(), String> {
    HomeOn::of(state, home_id)?.delete_profile(name).await?;

    let mut cfg = state.config.lock().unwrap();
    for inst in cfg.instances.iter_mut() {
        if inst.home_id == home_id {
            if inst.default_profile.as_deref() == Some(name) {
                inst.default_profile = None;
            }
            if inst.last_profile.as_deref() == Some(name) {
                inst.last_profile = None;
            }
        }
    }
    save_state(state, &cfg)?;

    Ok(())
}

/// Recursively copies a directory tree.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Instance runtime
// ---------------------------------------------------------------------------

/// Resolves (home path, version dir, version string) for an instance.
fn resolve_instance_paths(
    state: &State<'_, AppState>,
    instance_id: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf, String), String> {
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
    let version = cfg
        .versions
        .iter()
        .find(|v| v.id == inst.version_id)
        .ok_or_else(|| "版本不存在".to_string())?;
    Ok((
        home.path.clone(),
        version.dir.clone(),
        version.version.clone(),
    ))
}

/// Dependency-tree preflight for an instance + profile. Advisory only: the
/// report never blocks a launch, it is logged and handed to the UI.
#[tauri::command(rename_all = "snake_case")]
pub fn check_instance_health(
    state: State<'_, AppState>,
    instance_id: String,
    profile: String,
) -> Result<crate::doctor::DoctorReport, String> {
    let (home_path, version_dir, version) = resolve_instance_paths(&state, &instance_id)?;
    // The self-check reads package documents out of the version tree and the
    // profile — both on the machine that owns the instance. Another machine's
    // are not on this Mac, and a local read aimed at a remote path reports on
    // whatever happens to share that spelling (ADR-0006), so a remote instance
    // reports nothing instead of something wrong.
    if !instance_is_local(&state, &instance_id)? {
        return Ok(crate::doctor::DoctorReport {
            instance_id,
            profile,
            findings: Vec::new(),
        });
    }
    let profile_dir = crate::profile::profile_dir(&home_path, &profile)?;
    let report =
        crate::doctor::inspect(&instance_id, &profile, &version_dir, &version, &profile_dir);
    crate::doctor::log_report(&report);
    Ok(report)
}

/// Whether the files an instance runs on — its version tree and its profiles —
/// are on this Mac.
fn instance_is_local(state: &State<'_, AppState>, instance_id: &str) -> Result<bool, String> {
    let cfg = state.config.lock().unwrap();
    let inst = cfg
        .instances
        .iter()
        .find(|i| i.id == instance_id)
        .ok_or_else(|| "实例不存在".to_string())?;
    Ok(crate::config::resolve_host(&cfg, Some(&inst.host_id))?.builtin)
}

#[tauri::command]
pub async fn start_instance(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    profile: String,
) -> Result<(), String> {
    // Preflight the dependency tree before spawning: a duplicated core copy
    // in the profile breaks every tool call at runtime with no load-time
    // error, so it is reported up front instead of being debugged later.
    // Findings never block the launch. The version tree and the profile are the
    // instance's own machine's, so a remote instance is not checked from here
    // (see [`check_instance_health`]).
    if instance_is_local(&state, &id).unwrap_or(true) {
        if let Ok((home_path, version_dir, version)) = resolve_instance_paths(&state, &id) {
            let report = crate::doctor::inspect(
                &id,
                &profile,
                &version_dir,
                &version,
                &crate::profile::profile_dir(&home_path, &profile)?,
            );
            crate::doctor::log_report(&report);
            if !report.findings.is_empty() {
                use tauri::Emitter;
                let _ = app.emit(crate::doctor::HEALTH_EVENT, &report);
            }
        }
    }

    process::start_instance_process(&app, &state, &id, &profile).await?;
    // Remember the last used profile.
    let mut cfg = state.config.lock().unwrap();
    if let Some(inst) = cfg.instances.iter_mut().find(|i| i.id == id) {
        inst.last_profile = Some(profile);
    }
    save_state(&state, &cfg)
}

#[tauri::command]
pub async fn stop_instance(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    process::stop_instance_process(&app, &state, &id).await
}

/// Restarts a running instance on the profile it was running. One lifecycle
/// transition backend-side; the frontend used to compose stop + start itself,
/// which raced the stop's reap window against the start's preflight.
#[tauri::command]
pub async fn restart_instance(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    process::restart_instance_process(&app, &state, &id).await
}

#[tauri::command]
pub async fn list_instance_status(
    state: State<'_, AppState>,
) -> Result<Vec<crate::config::InstanceStatus>, String> {
    Ok(process::list_statuses(&state).await)
}

/// The page's one retry: ask the instance's own machine again what state it is
/// in, and report what it says.
///
/// It exists because 「主机不可达」 is a statement about a moment — a laptop that
/// went to sleep, a server that was rebooting — and the launcher must never
/// turn 「问不到」 into 「已停止」, which is the reading that leads to a second DSH
/// on that server (ADR-0007 后果). 本机 needs nothing: its instances are this
/// process's own children and their state is never unknown.
#[tauri::command]
pub async fn refresh_instance_status(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let found = {
        let cfg = state.config.lock().unwrap();
        cfg.instances
            .iter()
            .find(|inst| inst.id == id)
            .cloned()
            .map(|inst| {
                crate::config::resolve_host(&cfg, Some(&inst.host_id))
                    .map(|host| (inst, host.clone()))
            })
    };
    match found {
        None => Err("实例不存在".to_string()),
        Some(Err(e)) => Err(e),
        Some(Ok((_inst, host))) if host.builtin => Ok(()),
        Some(Ok((inst, host))) => {
            process::settle_remote_instance(&app, &state, &inst, &host).await;
            Ok(())
        }
    }
}

#[tauri::command]
pub async fn open_instance_window(
    _app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    // What gets opened is the address **this Mac** can reach: a 本机 instance's
    // own URL, or a remote instance's address brought over on the launcher's
    // port forward — carrying the launch token unchanged, so one click lands
    // logged in (ADR-0008 §3). DSH prints its own address from its own point of
    // view (`127.0.0.1` on that machine), which on this Mac is a different
    // service, and the token in its query string would be handed to whatever
    // answers there.
    let url = process::reachable_url_for(&state, &id).await?;
    // 实例页改由系统浏览器承载：复用 open_external 的 http(s) 校验与日志。
    open_external(url)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<LauncherSettings, String> {
    Ok(state.config.lock().unwrap().settings.clone())
}

/// The launcher's data directory (`<data_dir>`); the frontend shows it in
/// the settings page next to the "open directory" button.
#[tauri::command]
pub fn get_launcher_directory(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.data_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: SettingsPatch,
) -> Result<LauncherSettings, String> {
    let mut cfg = state.config.lock().unwrap();
    if let Some(v) = settings.locale {
        cfg.settings.locale = v;
    }
    if let Some(v) = settings.minimize_to_tray {
        cfg.settings.minimize_to_tray = v;
    }
    if let Some(v) = settings.autostart {
        let prev = cfg.settings.autostart;
        cfg.settings.autostart = v;
        if v != prev {
            use tauri_plugin_autostart::ManagerExt;
            let mgr = app.autolaunch();
            let result = if v { mgr.enable() } else { mgr.disable() };
            if let Err(e) = result {
                // Revert the stored flag so the UI stays truthful.
                cfg.settings.autostart = prev;
                return Err(format!("设置开机自启失败: {e}"));
            }
        }
    }
    if let Some(v) = settings.last_instance_id {
        cfg.settings.last_instance_id = Some(v);
    }
    if let Some(v) = settings.theme {
        match v.as_str() {
            "light" | "dark" | "system" => cfg.settings.theme = v,
            _ => return Err(format!("无效的主题: {v}")),
        }
    }
    if let Some(v) = settings.log_level {
        match crate::applog::parse_level(&v) {
            Some(level) => {
                cfg.settings.log_level = v.trim().to_ascii_lowercase();
                crate::applog::set_level(level);
                crate::log_info!("日志等级已切换为 {}", level.as_str());
            }
            None => return Err(format!("无效的日志等级: {v}")),
        }
    }
    if let Some(v) = settings.proxy_enabled {
        cfg.settings.proxy_enabled = v;
    }
    if let Some(v) = settings.proxy_url {
        let v = v.trim().trim_end_matches('/').to_string();
        if !v.is_empty() {
            cfg.settings.proxy_url = v;
        }
    }
    if let Some(v) = settings.proxy_port {
        cfg.settings.proxy_port = v;
    }
    if let Some(v) = settings.no_proxy {
        cfg.settings.no_proxy = v.trim().to_string();
    }
    if let Some(v) = settings.proxy_apply_dsh {
        cfg.settings.proxy_apply_dsh = v;
    }
    if let Some(v) = settings.terminal {
        match v.as_str() {
            "system" | "ghostty" => cfg.settings.terminal = v,
            _ => return Err(format!("无效的终端: {v}")),
        }
    }
    crate::proxy::sync_from_settings(&cfg.settings);
    let out = cfg.settings.clone();
    save_state(&state, &cfg)?;
    crate::log_debug!("设置已更新并保存");
    Ok(out)
}

// ---------------------------------------------------------------------------
// External links / directories
// ---------------------------------------------------------------------------

/// Opens an http(s) URL in the system browser. The Tauri webview ignores
/// `target="_blank"` anchors, so external links must go through here.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    let url = url.trim().to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("仅允许打开 http(s) 链接: {url}"));
    }
    crate::log_info!("在系统浏览器打开 {url}");
    open::that(&url).map_err(|e| format!("打开链接失败: {e}"))
}

/// Opens an external terminal (Terminal.app or Ghostty per settings) for one
/// instance: cwd at the instance's DSH_HOME, DSH_HOME and
/// DSH_LAUNCHER_INSTANCE injected, plus the instance's env overrides.
///
/// A local instance gets a `scripts/term_<id>.sh` file and a terminal that runs
/// it. An instance on another machine gets the same session over ssh instead
/// ([`remote_terminal_session`]): one command line, no file, and the machine is
/// asked first so an unreachable host is a sentence in the page rather than a
/// window that opens and dies.
#[tauri::command]
pub async fn open_instance_terminal(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<String, String> {
    let (cfg_home, cfg_inst, cfg_ver, cfg_host) = {
        let cfg = state.config.lock().unwrap();
        let inst = cfg
            .instances
            .iter()
            .find(|i| i.id == instance_id)
            .cloned()
            .ok_or_else(|| "实例不存在".to_string())?;
        let home = cfg
            .homes
            .iter()
            .find(|h| h.id == inst.home_id)
            .cloned()
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        let ver = cfg
            .versions
            .iter()
            .find(|v| v.id == inst.version_id)
            .cloned();
        let host = crate::config::resolve_host(&cfg, Some(&home.host_id))?.clone();
        (home, inst, ver, host)
    };
    let terminal = state.config.lock().unwrap().settings.terminal.clone();

    if !cfg_host.builtin {
        let session =
            remote_terminal_session(&state, &cfg_host, &cfg_inst, &cfg_home, cfg_ver.as_ref())
                .await?;
        return open_terminal_with(&terminal, &session.label, &session.line);
    }

    std::fs::create_dir_all(&cfg_home.path).map_err(|e| format!("创建 HOME 目录失败: {e}"))?;
    let home_str = cfg_home.path.to_string_lossy().to_string();

    // Prepend the instance's DSH version bin directory to PATH so `dsh` is
    // immediately available; the launch module owns the wrapper + PATH policy.
    let version_dir: Option<std::path::PathBuf> = cfg_ver
        .as_ref()
        .map(|v| std::path::PathBuf::from(&v.dir));
    let path_dirs = crate::launch::terminal_path_dirs(version_dir.as_deref());

    let path_prefix = if !path_dirs.is_empty() {
        let joined = path_dirs
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(":");
        format!("export PATH=\"{}:$PATH\"\n", joined)
    } else {
        String::new()
    };

    let env_pairs = crate::launch::terminal_env_pairs(
        &cfg_home.path,
        &cfg_inst.name,
        &cfg_inst.env_overrides,
    );
    let exports = env_pairs
        .iter()
        .map(|(k, v)| format!("export {}={}\n", k, shell_quote(v)))
        .collect::<Vec<_>>()
        .join("");

    // Create a dedicated launcher script for this instance's terminal session
    let term_script = local_terminal_script_path(&state.data_dir, &instance_id);
    if let Some(dir) = term_script.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let shell = shell_program();
    let script_content = format!(
        "#!/bin/sh\n\
        {path_prefix}\
        {exports}\
        cd {}\n\
        clear\n\
        exec {} -l\n",
        shell_quote(&home_str),
        shell
    );
    if std::fs::write(&term_script, script_content).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&term_script, std::fs::Permissions::from_mode(0o755));
        }
    }

    let label = format!("DSH {} ({})", cfg_inst.name, home_str);
    open_terminal_with(
        &terminal,
        &label,
        &format!("exec {}", shell_quote(&term_script.to_string_lossy())),
    )
}

/// The script a local instance's terminal session runs:
/// `<数据目录>/scripts/term_<实例 id>.sh`.
pub(crate) fn local_terminal_script_path(
    data_dir: &std::path::Path,
    instance_id: &str,
) -> std::path::PathBuf {
    data_dir
        .join("scripts")
        .join(format!("term_{instance_id}.sh"))
}

/// What a terminal window on this Mac needs to open an instance session: the
/// label the window carries, and the one command line it runs.
#[derive(Clone, Debug)]
pub(crate) struct TerminalSession {
    pub label: String,
    pub line: String,
}

/// The remote twin of a local instance terminal (user story 33): one ssh
/// command line that lands in the instance's DSH_HOME **on that machine**.
///
/// The machine is asked once, up front, so a host that is down is a readable
/// sentence in the page — and a DSH_HOME that is gone there is reported before
/// the terminal tries to `cd` into it. Nothing is written on this Mac: unlike
/// the local session, there is no script file to hand over.
pub(crate) async fn remote_terminal_session(
    state: &AppState,
    host: &Host,
    inst: &DshInstance,
    home: &DshHome,
    version: Option<&crate::config::DshVersion>,
) -> Result<TerminalSession, String> {
    let remote = crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;
    let home_str = home.path.to_string_lossy().to_string();
    state
        .host_transport
        .list_dir(&remote, &home_str)
        .await
        .map_err(|e| format!("无法在「{}」上打开终端：{e}", host.display_name))?;

    // The version's own `.bin` on that machine, first on PATH, so a bare `dsh`
    // in the session is the CLI the launcher drives.
    let path_dirs = version
        .map(|v| crate::launch::version_bin_dir(std::path::Path::new(&v.dir)))
        .map(|dir| vec![dir.to_string_lossy().to_string()])
        .unwrap_or_default();
    let env = crate::launch::terminal_env_pairs(&home.path, &inst.name, &inst.env_overrides);
    let script = crate::remote::terminal_script(&home_str, &env, &path_dirs);

    Ok(TerminalSession {
        label: format!(
            "DSH {} ({} → {})",
            inst.name, host.display_name, home_str
        ),
        line: crate::remote::terminal_command(host.ssh_target.trim(), &script),
    })
}

/// Hands one interactive shell command line to the terminal the user chose in
/// settings, and answers with the label the window carries. The local session
/// (a script file on this Mac) and the remote one (an ssh line) come through
/// here, so each terminal's own quirks live in one place.
fn open_terminal_with(terminal: &str, label: &str, line: &str) -> Result<String, String> {
    if terminal == "ghostty" {
        // Ghostty on macOS: -na opens a new instance and passes --command to execute the command.
        let status = std::process::Command::new("open")
            .arg("-na")
            .arg("Ghostty")
            .arg("--args")
            .arg(format!("--title={label}"))
            .arg(format!("--command={}", shell_quote(line)))
            .spawn()
            .and_then(|mut c| c.wait());

        // Bring Ghostty to front
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg("tell application \"Ghostty\" to activate")
            .output();

        match status {
            Ok(_) => Ok(label.to_string()),
            Err(e) => Err(format!("打开 Ghostty 失败: {e}")),
        }
    } else {
        // Terminal.app. A cold-started Terminal always opens a default
        // startup window, so `activate` + `do script` would produce two
        // windows. Instead, on cold start we wait for that startup window
        // and run the command inside it.
        let escaped = line.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "tell application \"Terminal\"\n\
            set wasRunning to running\n\
            if not wasRunning then\n\
            activate\n\
            repeat 20 times\n\
            if (count of windows) > 0 then exit repeat\n\
            delay 0.1\n\
            end repeat\n\
            if (count of windows) > 0 then\n\
            do script \"{escaped}\" in tab 1 of front window\n\
            return\n\
            end if\n\
            end if\n\
            do script \"{escaped}\"\n\
            activate\n\
            end tell"
        );
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("打开终端失败: {e}"))?;
        if out.status.success() {
            Ok(label.to_string())
        } else {
            Err(format!(
                "打开终端失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// The login shell for spawned terminals: $SHELL or /bin/zsh.
fn shell_program() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// Single-quote a value for sh -c / do-script embedding. The same rule
/// [`crate::remote::quote`] applies on the way to another machine, and the one
/// [`crate::remote::terminal_command`] applies a second time for the shell that
/// parses the command line here.
pub(crate) fn shell_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "'\\''"))
}

/// Opens a folder in the system file manager. On macOS this reuses an
/// existing Finder tab that already shows the folder (bringing its window
/// to front and selecting that tab); otherwise it opens a new tab in the
/// frontmost window. Creating a tab requires synthetic keystrokes, so when
/// the accessibility permission is missing it falls back to a new window.
fn open_folder_in_file_manager(dir: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Canonicalize so the comparison matches Finder's POSIX path
        // (resolves /tmp -> /private/tmp and other symlinks).
        let canon = dir
            .canonicalize()
            .unwrap_or_else(|_| dir.to_path_buf());
        let posix = format!("{}/", canon.to_string_lossy().trim_end_matches('/'));
        let script = r#"on run argv
    set posixPath to item 1 of argv
    tell application "Finder"
        repeat with i from 1 to count of Finder windows
            set w to Finder window i
            try
                if ((POSIX path of ((target of w) as alias)) as string) = posixPath then
                    set index of w to 1
                    set collapsed of w to false
                    activate
                    return "reused"
                end if
            end try
        end repeat
        activate
        delay 0.2
        if (count of Finder windows) is 0 then
            make new Finder window to (POSIX file (text 1 thru -2 of posixPath))
            return "new-window"
        end if
        try
            tell application "System Events" to keystroke "t" using command down
            delay 0.3
            set target of front Finder window to (POSIX file (text 1 thru -2 of posixPath))
            return "new-tab"
        on error
            make new Finder window to (POSIX file (text 1 thru -2 of posixPath))
            return "new-window"
        end try
    end tell
end run"#;
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .arg("--")
            .arg(&posix)
            .output()
            .map_err(|e| format!("打开目录失败: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        Err(format!(
            "打开目录失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        open::that(dir).map_err(|e| format!("打开目录失败: {e}"))
    }
}

/// Opens the file manager at a log file with the file selected (Windows
/// Explorer, macOS Finder). Falls back to opening the file itself on other
/// platforms. Returns the resolved log path so the UI can show it.
fn reveal_log_file(
    _app: &tauri::AppHandle,
    log_path: std::path::PathBuf,
) -> Result<String, String> {
    let path_str = log_path.to_string_lossy().to_string();
    if log_path.exists() {
        crate::log_info!("在访达中定位日志文件 {path_str}");
        let status = std::process::Command::new("open")
            .arg("-R")
            .arg(&log_path)
            .spawn()
            .and_then(|mut c| c.wait());
        if status.is_ok() {
            return Ok(path_str);
        }
        crate::log_warn!("open -R 失败，改用默认打开方式");
        open::that(&log_path).map_err(|e| format!("打开日志文件失败: {e}"))?;
        Ok(path_str)
    } else {
        // The log file does not exist yet: open the log directory instead.
        let dir = log_path
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_default();
        crate::log_info!("日志文件不存在，打开日志目录 {}", dir.display());
        open::that(&dir).map_err(|e| format!("打开日志目录失败: {e}"))?;
        Ok(dir.to_string_lossy().to_string())
    }
}

/// Opens the launcher's own data directory (config, homes, logs, …).
#[tauri::command]
pub fn open_launcher_directory(state: State<'_, AppState>) -> Result<String, String> {
    let dir = state.data_dir.clone();
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    crate::log_info!("在文件管理器中打开启动器数据目录 {}", dir.display());
    open_folder_in_file_manager(&dir)?;
    Ok(dir.to_string_lossy().to_string())
}

/// Reveals the launcher runtime log (`<data_dir>/logs/latest.log`) in the
/// file manager with the file selected, creating the directory when needed.
#[tauri::command]
pub fn open_launcher_log(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let log_dir = state.data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
    reveal_log_file(&app, log_dir.join("latest.log"))
}

/// Reveals one instance's runtime log in the file manager with the file
/// selected: `<data_dir>/logs/<instance_id>.log` for an instance on this Mac,
/// and a copy of the tail of that machine's log for one elsewhere (user story
/// 34). `open -R` needs a local file, so the remote log is fetched first.
#[tauri::command]
pub async fn open_instance_log(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<String, String> {
    match instance_log_source(&state, &instance_id).await? {
        InstanceLog::Local(log_path) => {
            let log_dir = state.data_dir.join("logs");
            std::fs::create_dir_all(&log_dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
            reveal_log_file(&app, log_path)
        }
        InstanceLog::Remote { host, log_path } => {
            let local = fetch_remote_log(&state, &host, &log_path, &instance_id).await?;
            reveal_log_file(&app, local)
        }
    }
}

/// Where one instance's log is read from: the launcher's own log directory for
/// an instance on this Mac, or the log file that instance writes **on its own
/// machine** for one elsewhere.
///
/// The path is the one the remote run carries in
/// [`crate::process::RemoteRun::log_path`] when the instance is running, and the
/// same derivation when it is not — a stopped instance's log is still there to
/// read, exactly as a local one's is.
pub(crate) enum InstanceLog {
    Local(std::path::PathBuf),
    /// The machine the instance runs on (boxed: a host record is far larger
    /// than the other arm), and the path of its log **there**.
    Remote {
        host: Box<Host>,
        log_path: String,
    },
}

pub(crate) async fn instance_log_source(
    state: &AppState,
    instance_id: &str,
) -> Result<InstanceLog, String> {
    let host = {
        let cfg = state.config.lock().unwrap();
        let inst = cfg
            .instances
            .iter()
            .find(|i| i.id == instance_id)
            .ok_or_else(|| "实例不存在".to_string())?;
        let home_id = cfg
            .homes
            .iter()
            .find(|h| h.id == inst.home_id)
            .map(|h| h.host_id.clone())
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        crate::config::resolve_host(&cfg, Some(&home_id))?.clone()
    };
    if host.builtin {
        return Ok(InstanceLog::Local(
            state
                .data_dir
                .join("logs")
                .join(format!("{instance_id}.log")),
        ));
    }
    let data_dir = host.data_dir(&state.data_dir)?;
    let from_run = state
        .running
        .lock()
        .await
        .get(instance_id)
        .and_then(|entry| entry.remote.as_ref())
        .map(|run| run.log_path.clone());
    Ok(InstanceLog::Remote {
        host: Box::new(host),
        log_path: from_run.unwrap_or_else(|| crate::process::remote_log_path(&data_dir, instance_id)),
    })
}

/// Fetches the last stretch of one remote instance's log into a local file and
/// answers with where it landed, so the file manager can reveal it the way it
/// reveals a local log.
///
/// The copy is written under the launcher's own log directory and overwritten on
/// every open: it is a copy of the tail, not a second log to follow.
pub(crate) async fn fetch_remote_log(
    state: &AppState,
    host: &Host,
    log_path: &str,
    instance_id: &str,
) -> Result<std::path::PathBuf, String> {
    let remote = crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;
    let tail = crate::remote::log_tail(
        &*state.host_transport,
        &remote,
        log_path,
        crate::remote::LOG_TAIL_BYTES,
    )
    .await
    .map_err(|e| {
        format!(
            "读取「{}」上的日志失败（{log_path}）：{e}",
            host.display_name
        )
    })?;
    let local = state
        .data_dir
        .join("logs")
        .join(format!("{instance_id}.remote.log"));
    if let Some(dir) = local.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
    }
    std::fs::write(&local, tail.text.as_bytes()).map_err(|e| format!("写入日志副本失败: {e}"))?;
    Ok(local)
}

/// Where one instance lives on another machine, and how a user reaches it from
/// this Mac: the path to paste, and one runnable command.
#[derive(Clone, Debug, serde::Serialize)]
pub struct InstanceLocation {
    pub host_id: String,
    pub host_name: String,
    pub ssh_target: String,
    /// The DSH_HOME as that machine spells it.
    pub path: String,
    /// `ls -la` at that path, prefixed to run through ssh. Put together by
    /// [`crate::remote::ssh_command`], which owns the quoting a command the
    /// user pastes into their own shell needs.
    pub command: String,
}

/// The remote instance's location, for the card's copy actions.
///
/// A remote DSH_HOME cannot be opened in this Mac's file manager, and a button
/// that reaches nothing is worse than none (ADR-0006): the path and a command
/// are what the user can actually use. This answers from the launcher's records
/// alone, so it works while that machine is off.
#[tauri::command]
pub fn instance_remote_location(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<InstanceLocation, String> {
    instance_location_for(&state, &instance_id)
}

pub(crate) fn instance_location_for(
    state: &AppState,
    instance_id: &str,
) -> Result<InstanceLocation, String> {
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
    let host = crate::config::resolve_host(&cfg, Some(&home.host_id))?;
    if host.builtin {
        return Err("本机实例的目录就在这台 Mac 上，用「打开目录」".to_string());
    }
    let target = host.ssh_target.trim();
    if target.is_empty() {
        return Err(format!("「{}」还没有 ssh 目标", host.display_name));
    }
    let path = home.path.to_string_lossy().to_string();
    Ok(InstanceLocation {
        host_id: host.id.clone(),
        host_name: host.display_name.clone(),
        ssh_target: target.to_string(),
        command: crate::remote::ssh_command(target, &directory_recipe(&path)),
        path,
    })
}

/// What a user runs on this Mac to look at a remote directory. The path goes in
/// single quotes so the login shell at the other end sees it as one word;
/// [`crate::remote::ssh_command`] owns the rest of the quoting.
fn directory_recipe(path: &str) -> String {
    format!("ls -la {}", crate::remote::quote(path))
}

/// Opens the DSH_HOME directory of one instance in the file manager.
///
/// Deliberately local-only: a remote instance's HOME is a directory on that
/// server, and this opens this Mac's file manager. The card offers the copy
/// actions behind `instance_remote_location` for those instead, and this refuses
/// with the reason.
#[tauri::command]
pub fn open_instance_directory(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<String, String> {
    let home = {
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
        // A remote instance's HOME is a directory on that server: creating it
        // here would leave a stray directory on this Mac at the same spelling,
        // and opening it in the file manager would show the wrong machine.
        crate::config::resolve_host(&cfg, Some(&home.host_id))?.local_paths_only("DSH_HOME 目录")?;
        home.path.clone()
    };
    if !home.is_dir() {
        std::fs::create_dir_all(&home).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    crate::log_info!(
        "在文件管理器中打开实例 {} 的 DSH_HOME {}",
        instance_id,
        home.display()
    );
    open_folder_in_file_manager(&home)?;
    Ok(home.to_string_lossy().to_string())
}

/// Opens a DSH_HOME directory in the file manager.
///
/// Local-only for the same reason as the instance one above: a remote HOME lives
/// on that server, and the DSH_HOME list offers 「复制路径」 for it instead.
#[tauri::command]
pub fn open_home_directory(
    state: State<'_, AppState>,
    home_id: String,
) -> Result<String, String> {
    let home = {
        let cfg = state.config.lock().unwrap();
        let home = cfg
            .homes
            .iter()
            .find(|h| h.id == home_id)
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        // Same rule as the instance one: a remote HOME is that machine's
        // directory, and this Mac must not be made to have a copy of its path.
        crate::config::resolve_host(&cfg, Some(&home.host_id))?.local_paths_only("DSH_HOME 目录")?;
        home.path.clone()
    };
    if !home.is_dir() {
        std::fs::create_dir_all(&home).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    crate::log_info!("在文件管理器中打开 DSH_HOME {}", home.display());
    open_folder_in_file_manager(&home)?;
    Ok(home.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

pub(crate) fn save_state(state: &AppState, cfg: &crate::config::Config) -> Result<(), String> {
    crate::config::save_config(&state.config_path, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch data dir under the OS temp dir (the existing test style),
    /// removed on drop so a failed assert never leaves debris behind.
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

    /// The Settings page's host list: a config written before hosts existed
    /// still has the built-in local host in it, and only that one.
    #[test]
    fn list_hosts_reports_the_built_in_local_host_for_an_old_config() {
        let data = ScratchDir::new("list-hosts");
        let config_path = data.path().join("config.json");
        std::fs::write(&config_path, r#"{"homes":[],"settings":{"locale":"zh-CN"}}"#).unwrap();

        // The config `list_hosts` answers from.
        let cfg = crate::config::load_config(&config_path);
        let hosts = cfg.hosts;

        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].id, crate::config::LOCAL_HOST_ID);
        assert!(hosts[0].builtin, "本机是内置的那一条");
        assert_eq!(hosts[0].display_name, "本机");
        assert!(hosts[0].ssh_target.is_empty(), "本机不经过 ssh");
    }

    // -----------------------------------------------------------------------
    // Hosts
    // -----------------------------------------------------------------------

    /// A remote host record with a data directory on that machine.
    fn server_host(id: &str, registry: Option<&str>) -> Host {
        let mut host = remote_host(id, "zjq_aliyun");
        host.data_dir = Some(std::path::PathBuf::from(
            "/home/demo/.local/share/dsh-launcher",
        ));
        host.registry = registry.map(str::to_string);
        host
    }

    /// Which machine's npm answers the installable-version question, and with
    /// which registry: 本机 with its own record's registry, a remote host with
    /// its own registry asked **there** (a mirror that only that machine can
    /// reach would otherwise offer a list it cannot install), and a remote host
    /// without one asked here — the same registry, so no ssh and no second way
    /// to fail.
    #[test]
    fn a_hosts_installable_list_is_queried_from_its_own_registry() {
        assert_eq!(
            versions_source(&Host::local()),
            VersionsSource::Local { registry: None },
            "本机用启动器自己这台机器的 npm"
        );

        let local_with_registry = Host {
            registry: Some("https://npm.example.com/".into()),
            ..Host::local()
        };
        assert_eq!(
            versions_source(&local_with_registry),
            VersionsSource::Local {
                registry: Some("https://npm.example.com/".into())
            },
            "本机记录里写了 registry 就按它查"
        );

        assert_eq!(
            versions_source(&server_host("host-lab", None)),
            VersionsSource::Local { registry: None },
            "远程主机没配 registry 时复用本机那份"
        );
        assert_eq!(
            versions_source(&server_host("host-lab", Some("https://npm.example.com"))),
            VersionsSource::Remote {
                registry: "https://npm.example.com".into()
            }
        );
        // A registry field holding only spaces is no registry at all.
        assert_eq!(
            versions_source(&server_host("host-lab", Some("   "))),
            VersionsSource::Local { registry: None }
        );
    }

    /// npm's two `--json` answers become the list the page shows: the version
    /// list decides the entries, the timing map fills in the dates it knows.
    /// npm >= 9 wraps `time` in a one-element array; older npm returned the
    /// object directly — both shapes are read, because both are in the field.
    #[test]
    fn npm_listings_become_versions_with_their_dates() {
        let flat = versions_from_npm(
            "[\"0.1.0-rc.6\",\"0.1.0-rc.5\"]",
            "{\"created\":\"2026-01-01T00:00:00Z\",\"0.1.0-rc.6\":\"2026-08-01T12:00:00Z\"}",
        )
        .unwrap();
        assert_eq!(
            flat.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(),
            vec!["0.1.0-rc.6", "0.1.0-rc.5"]
        );
        assert_eq!(flat[0].released_at.as_deref(), Some("2026-08-01T12:00:00Z"));
        assert_eq!(flat[1].released_at, None, "没有发布时间的版本也要列出来");

        let wrapped = versions_from_npm(
            "[\"0.1.0-rc.6\"]",
            "[{\"0.1.0-rc.6\":\"2026-08-01T12:00:00Z\"}]",
        )
        .unwrap();
        assert_eq!(wrapped[0].released_at.as_deref(), Some("2026-08-01T12:00:00Z"));

        assert!(
            versions_from_npm("not json", "{}").is_err(),
            "读不懂的答案要报错，不能当成空列表"
        );
    }

    /// What a remote host has installed comes back the way **that machine**
    /// describes it: the records say which versions the user installed there,
    /// and the machine says where each one really is and whether its bin is
    /// ready. A version whose install stopped early is listed, marked as not
    /// ready — it is a record the user has to be able to see and delete, not a
    /// version anyone can start.
    #[test]
    fn a_remote_hosts_installed_versions_carry_their_path_and_bin_readiness() {
        let data = ScratchDir::new("remote-versions");
        let host = server_host("host-lab", None);
        let remote_data = "/home/demo/.local/share/dsh-launcher";
        let mut cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            versions: vec![
                crate::config::DshVersion {
                    id: "v-rc6".into(),
                    version: "0.1.0-rc.6".into(),
                    dir: format!("{remote_data}/versions/0.1.0-rc.6").into(),
                    host_id: host.id.clone(),
                },
                crate::config::DshVersion {
                    id: "v-rc5".into(),
                    version: "0.1.0-rc.5".into(),
                    dir: format!("{remote_data}/versions/0.1.0-rc.5").into(),
                    host_id: host.id.clone(),
                },
            ],
            ..Default::default()
        };
        // A local version, which the remote list must not pick up.
        cfg.versions.push(crate::config::DshVersion {
            id: "v-local".into(),
            version: "0.1.0-rc.4".into(),
            dir: data.path().join("versions/0.1.0-rc.4"),
            host_id: crate::config::LOCAL_HOST_ID.into(),
        });

        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::marked(
                "{\"versions\":[\
                 {\"name\":\"0.1.0-rc.6\",\"dir\":\"/home/demo/.local/share/dsh-launcher/versions/0.1.0-rc.6\",\"bin_ready\":true},\
                 {\"name\":\"0.1.0-rc.5\",\"dir\":\"/home/demo/.local/share/dsh-launcher/versions/0.1.0-rc.5\",\"bin_ready\":false}]}",
            )),
        );
        let state = host_state(&data, cfg, fake.clone());

        let listed = block_on(list_versions_for(&state, Some("host-lab"))).unwrap();

        assert_eq!(
            listed.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(),
            vec!["0.1.0-rc.6", "0.1.0-rc.5"],
            "只列这台主机的记录"
        );
        assert!(listed[0].bin_ready, "机器说 rc.6 的 bin 就绪");
        assert!(!listed[1].bin_ready, "安装没走完：如实报不就绪");
        assert_eq!(listed[1].dir.to_string_lossy(), format!("{remote_data}/versions/0.1.0-rc.5"));
        assert_eq!(
            listed[0].dedicated_home,
            format!("{remote_data}/homes/0.1.0-rc.6"),
            "专属 DSH_HOME 按那台主机的数据目录算"
        );
        assert!(
            fake.last_script().contains(&format!("'{remote_data}/versions'")),
            "一次 ssh 问那台机器的 versions 目录：{}",
            fake.last_script()
        );
    }

    /// A machine the launcher cannot reach is an error, never an empty list:
    /// 「这台机器上没有版本」 is a claim only the machine can make, and a page that
    /// shows it after a failed call is telling the user something it does not
    /// know.
    #[test]
    fn an_unreachable_hosts_version_list_is_an_error_not_an_empty_list() {
        let data = ScratchDir::new("remote-versions-down");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            versions: vec![crate::config::DshVersion {
                id: "v-rc6".into(),
                version: "0.1.0-rc.6".into(),
                dir: "/home/demo/.local/share/dsh-launcher/versions/0.1.0-rc.6".into(),
                host_id: host.id.clone(),
            }],
            ..Default::default()
        };
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::fail(
                crate::ssh::HostErrorKind::Unreachable,
                "ssh: connect to host zjq_aliyun port 22: Connection refused",
            ),
        );
        let state = host_state(&data, cfg, fake);

        let err = block_on(list_versions_for(&state, Some("host-lab"))).unwrap_err();

        assert!(err.contains("连不上"), "{err}");
        assert!(err.contains("Connection refused"), "{err}");
    }

    /// 本机's own list needs no ssh and answers bin-readiness from this
    /// filesystem: a version directory that never got its bin is reported the
    /// same way a remote one is.
    #[test]
    fn the_local_version_list_reports_bin_readiness_from_this_machine() {
        let data = ScratchDir::new("local-versions");
        let ready = data.path().join("versions/0.1.0-rc.6");
        std::fs::create_dir_all(ready.join("node_modules/@deepseek-ai/dsh/lib")).unwrap();
        std::fs::write(
            ready.join("node_modules/@deepseek-ai/dsh/lib/bin.js"),
            "// dsh\n",
        )
        .unwrap();
        let half = data.path().join("versions/0.1.0-rc.5");
        std::fs::create_dir_all(&half).unwrap();

        let cfg = crate::config::Config {
            versions: vec![
                crate::config::DshVersion {
                    id: "v-rc6".into(),
                    version: "0.1.0-rc.6".into(),
                    dir: ready.clone(),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
                crate::config::DshVersion {
                    id: "v-rc5".into(),
                    version: "0.1.0-rc.5".into(),
                    dir: half.clone(),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
            ],
            ..Default::default()
        };
        let state = host_state(&data, cfg, crate::remote::testing::idle());

        let listed = block_on(list_versions_for(&state, None)).unwrap();

        assert!(listed[0].bin_ready);
        assert!(!listed[1].bin_ready, "只有目录、没有 bin：不算装好");
        assert_eq!(listed[0].dedicated_home, dedicated_home_path(data.path(), "0.1.0-rc.6").to_string_lossy());
    }

    /// The board shows every host's instances by default, and that is an
    /// **explicit** request: no host named still means 本机, so a caller that
    /// never says which machine it wants never quietly gets two of them merged.
    /// Two instances called the same thing on two hosts stay two rows, each
    /// carrying its own host — that is what keeps them apart on the board.
    #[test]
    fn the_all_hosts_request_lists_every_host_and_silence_still_means_local() {
        let data = ScratchDir::new("all-instances");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            instances: vec![
                instance_record("i-local-web", "web", crate::config::LOCAL_HOST_ID),
                instance_record("i-remote-web", "web", &host.id),
                instance_record("i-remote-lab", "实验", &host.id),
            ],
            ..Default::default()
        };
        let state = host_state(&data, cfg, crate::remote::testing::idle());

        let local = list_instances_for(&state, None, false).unwrap();
        assert_eq!(
            local.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["i-local-web"],
            "没说主机就是本机"
        );

        let remote = list_instances_for(&state, Some("host-lab"), false).unwrap();
        assert_eq!(remote.len(), 2);

        let every = list_instances_for(&state, None, true).unwrap();
        assert_eq!(every.len(), 3, "显式要全部时才把所有主机的实例合在一起");
        // Two instances named 「web」 on two hosts are two distinguishable rows.
        let webs = every.iter().filter(|i| i.name == "web").collect::<Vec<_>>();
        assert_eq!(webs.len(), 2);
        assert_ne!(webs[0].host_id, webs[1].host_id);
    }

    fn instance_record(id: &str, name: &str, host_id: &str) -> crate::config::DshInstance {
        crate::config::DshInstance {
            id: id.into(),
            name: name.into(),
            version_id: "v1".into(),
            home_id: "h1".into(),
            host_id: host_id.into(),
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        }
    }

    /// A remote host with one DSH_HOME on it and one instance bound to it: the
    /// smallest configuration in which 「这个 HOME 的 Profile」 is a question
    /// about another machine.
    fn remote_home_cfg(host: &Host, path: &str) -> crate::config::Config {
        crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            homes: vec![DshHome {
                id: "h-lab".into(),
                name: "服务器默认".into(),
                path: std::path::PathBuf::from(path),
                host_id: host.id.clone(),
            }],
            instances: vec![crate::config::DshInstance {
                home_id: "h-lab".into(),
                default_profile: Some("web".into()),
                last_profile: Some("web".into()),
                ..instance_record("i-lab", "实验实例", &host.id)
            }],
            ..Default::default()
        }
    }

    fn remote_stdout(payload: &str) -> crate::remote::testing::FakeAnswer {
        crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::marked(payload))
    }

    /// What a remote write answers: the marker-wrapped acknowledgement every
    /// one of those scripts prints.
    fn remote_ack() -> crate::remote::testing::FakeAnswer {
        remote_stdout(r#"{"ok":true}"#)
    }

    /// A remote DSH_HOME's profiles are that machine's directories, read with
    /// one ssh — not this Mac's `/home/demo/.dsh/profiles`, which is a
    /// different directory that happens to share a spelling.
    #[test]
    fn a_remote_homes_profiles_are_read_from_that_machine() {
        let data = ScratchDir::new("remote-profiles");
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(remote_stdout(r#"{"profiles":["web","demo"]}"#));
        let state = host_state(
            &data,
            remote_home_cfg(&server_host("host-lab", None), "/home/demo/.dsh"),
            fake.clone(),
        );

        let listed = block_on(list_profiles_for(&state, "h-lab")).unwrap();

        assert_eq!(listed, vec!["demo", "web"], "和本机一样按名字排序");
        assert_eq!(fake.calls().len(), 1, "一份列表一次 ssh");
        let script = fake.last_script();
        assert!(
            script.contains("'/home/demo/.dsh/profiles'"),
            "读的是那台机器上的那个目录：{script}"
        );
    }

    /// 「重新读取 Profile」 is this read asked again: nothing is cached in the
    /// launcher, so a profile edited on the server shows up on the second look.
    #[test]
    fn rereading_a_remote_homes_profiles_asks_that_machine_again() {
        let data = ScratchDir::new("remote-profiles-reread");
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(remote_stdout(r#"{"profiles":["web"]}"#))
            .answering(remote_stdout(r#"{"profiles":["web","lab"]}"#));
        let state = host_state(
            &data,
            remote_home_cfg(&server_host("host-lab", None), "/home/demo/.dsh"),
            fake.clone(),
        );

        let first = block_on(list_profiles_for(&state, "h-lab")).unwrap();
        let second = block_on(list_profiles_for(&state, "h-lab")).unwrap();

        assert_eq!(first, vec!["web"]);
        assert_eq!(second, vec!["lab", "web"], "第二次读的是那台机器现在的答案");
        assert_eq!(fake.calls().len(), 2);
    }

    /// Copying a profile on a remote DSH_HOME is one command on that machine,
    /// naming both ends as paths there — the launcher has no local copy of the
    /// tree to copy from.
    #[test]
    fn copying_a_remote_profile_is_one_command_on_that_machine() {
        let data = ScratchDir::new("remote-profile-copy");
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(remote_ack());
        let state = host_state(
            &data,
            remote_home_cfg(&server_host("host-lab", None), "/home/demo/.dsh"),
            fake.clone(),
        );

        let name = block_on(copy_profile_for(&state, "h-lab", "web", "my web")).unwrap();

        assert_eq!(name, "my web");
        assert_eq!(fake.calls().len(), 1, "一次 ssh 做完这次复制");
        let script = fake.last_script();
        assert!(
            script.contains("'/home/demo/.dsh/profiles/web'")
                && script.contains("'/home/demo/.dsh/profiles/my web'"),
            "两端都是那台机器上的路径：{script}"
        );
    }

    /// Renaming a profile that instances run with keeps their references in
    /// step, wherever the HOME is. The rename itself happens there; the records
    /// are the launcher's and move here.
    #[test]
    fn renaming_a_remote_profile_happens_there_and_follows_the_instances() {
        let data = ScratchDir::new("remote-profile-rename");
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(remote_ack());
        let state = host_state(
            &data,
            remote_home_cfg(&server_host("host-lab", None), "/home/demo/.dsh"),
            fake.clone(),
        );

        let renamed = block_on(rename_profile_for(&state, "h-lab", "web", "web2")).unwrap();

        assert_eq!(renamed, "web2");
        let script = fake.last_script();
        assert!(
            script.contains("'/home/demo/.dsh/profiles/web'")
                && script.contains("'/home/demo/.dsh/profiles/web2'"),
            "{script}"
        );
        let cfg = state.config.lock().unwrap();
        assert_eq!(
            cfg.instances[0].default_profile.as_deref(),
            Some("web2"),
            "实例的默认 Profile 跟着改名"
        );
        assert_eq!(cfg.instances[0].last_profile.as_deref(), Some("web2"));
    }

    /// Removing a remote profile removes the directory **there** (a profile is
    /// data on that machine, not a record), and clears the references to it.
    #[test]
    fn deleting_a_remote_profile_happens_there_and_clears_the_instances() {
        let data = ScratchDir::new("remote-profile-delete");
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(remote_ack());
        let state = host_state(
            &data,
            remote_home_cfg(&server_host("host-lab", None), "/home/demo/.dsh"),
            fake.clone(),
        );

        block_on(delete_profile_for(&state, "h-lab", "web")).unwrap();

        let script = fake.last_script();
        assert!(
            script.contains("'/home/demo/.dsh/profiles/web'"),
            "{script}"
        );
        let cfg = state.config.lock().unwrap();
        assert_eq!(cfg.instances[0].default_profile, None);
        assert_eq!(cfg.instances[0].last_profile, None);
    }

    /// The traversal guard holds on the far side too: a name that is not one
    /// path segment never becomes a path, and nothing is dialed to find out.
    #[test]
    fn a_remote_profile_name_that_would_escape_is_refused_before_any_ssh() {
        let data = ScratchDir::new("remote-profile-escape");
        let fake = crate::remote::testing::FakeTransport::new();
        let state = host_state(
            &data,
            remote_home_cfg(&server_host("host-lab", None), "/home/demo/.dsh"),
            fake.clone(),
        );

        let err = block_on(copy_profile_for(&state, "h-lab", "../../etc", "x")).unwrap_err();

        assert!(err.contains("名称不合法"), "{err}");
        assert!(fake.calls().is_empty(), "拒绝发生在拼路径的时候：{:?}", fake.calls());
    }

    /// 本机's profile operations are unchanged: the same files, directly, with
    /// no transport involved and nothing dialed.
    #[test]
    fn the_local_host_still_reads_and_changes_its_own_profiles() {
        let data = ScratchDir::new("local-profiles");
        let home_path = data.path().join("local-dsh");
        std::fs::create_dir_all(home_path.join("profiles/web")).unwrap();
        std::fs::write(home_path.join("profiles/web/package.json"), "{}").unwrap();
        let cfg = crate::config::Config {
            hosts: vec![Host::local()],
            homes: vec![DshHome {
                id: "h1".into(),
                name: "本机 HOME".into(),
                path: home_path.clone(),
                host_id: crate::config::LOCAL_HOST_ID.into(),
            }],
            ..Default::default()
        };
        let fake = crate::remote::testing::FakeTransport::new();
        let state = host_state(&data, cfg, fake.clone());

        let listed = block_on(list_profiles_for(&state, "h1")).unwrap();
        assert_eq!(listed, vec!["web"]);
        block_on(copy_profile_for(&state, "h1", "web", "web copy")).unwrap();
        assert!(home_path.join("profiles/web copy/package.json").is_file());
        block_on(rename_profile_for(&state, "h1", "web copy", "renamed")).unwrap();
        assert!(home_path.join("profiles/renamed").is_dir());
        block_on(delete_profile_for(&state, "h1", "renamed")).unwrap();
        assert!(!home_path.join("profiles/renamed").exists());
        assert!(home_path.join("profiles/web").is_dir());

        assert!(
            fake.calls().is_empty(),
            "本机的 Profile 操作不经过 ssh：{:?}",
            fake.calls()
        );
    }

    /// The path of a remote DSH_HOME is a path on that server, so the server is
    /// what settles it: the machine resolves `~`, creates the directory if it
    /// is missing, and answers where it landed. The record keeps **that**
    /// answer — never the raw text, and never a path this Mac came up with.
    ///
    /// The path is typed by hand for the server, so a space and a single quote
    /// in it are ordinary; the script that goes over ssh must carry it as one
    /// word through the local argv **and** the remote shell.
    #[test]
    fn a_remote_home_is_validated_on_that_machine_and_saved_with_its_answer() {
        let data = ScratchDir::new("remote-home");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            ..Default::default()
        };
        let typed = "/home/de mo/it's here/.dsh";
        let settled = "/home/de mo/it's here/.dsh";
        let answer = crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::marked(
            &format!(r#"{{"path":"{settled}"}}"#),
        ));
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(answer.clone())
            .answering(answer);
        let state = host_state(&data, cfg, fake.clone());

        let home = block_on(create_home_record_in(&state, "远端 HOME", typed, Some("host-lab")))
            .unwrap();

        assert_eq!(home.host_id, "host-lab");
        assert_eq!(home.path.to_string_lossy(), settled, "记录的是那台机器给的路径");
        assert!(
            !std::path::Path::new(typed).exists(),
            "这台 Mac 上不因为这条记录多出任何目录"
        );
        let script = fake.last_script();
        assert!(
            script.contains(&crate::remote::quote(typed)),
            "整条路径要作为一个词过两层 shell：{script}"
        );
        assert!(
            script.contains(r"'/home/de mo/it'\''s here/.dsh'"),
            "空格与单引号都要转义：{script}"
        );

        // Asking for the same directory again reuses the record it already has.
        let again = block_on(create_home_record_in(
            &state,
            "另一个名字",
            "/home/de mo/other/../it's here/.dsh",
            Some("host-lab"),
        ))
        .unwrap();
        assert_eq!(again.id, home.id, "同一台机器上同一个目录只有一条记录");
    }

    /// A path the machine will not use is refused **by that machine**, and
    /// nothing is saved: a record pointing at a directory that does not exist
    /// where the instance will run is worse than no record.
    #[test]
    fn a_remote_path_the_machine_refuses_saves_no_record() {
        let data = ScratchDir::new("remote-home-bad");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            ..Default::default()
        };
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::failed(
                3,
                "请填写绝对路径（以 / 开头）：relative/homes",
            ),
        );
        let state = host_state(&data, cfg, fake.clone());

        let err =
            block_on(create_home_record_in(&state, "远端 HOME", "relative/homes", Some("host-lab")))
                .unwrap_err();

        assert!(
            state.config.lock().unwrap().homes.is_empty(),
            "远端校验没过就不写记录：{err}"
        );
        assert!(err.contains("绝对路径"), "理由来自远端：{err}");
        assert_eq!(fake.calls().len(), 1, "只问了那台机器一次");
    }

    /// Path equality is the host's own: 本机's APFS volume folds case, a remote
    /// Linux host does not. The same spelling typed twice is one record on
    /// either machine; two spellings are one record here and two there.
    #[test]
    fn a_remote_path_with_different_case_is_a_different_path() {
        let data = ScratchDir::new("remote-case");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            ..Default::default()
        };
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::marked(r#"{"path":"/home/demo/.dsh"}"#),
            ))
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::marked(r#"{"path":"/home/demo/.DSH"}"#),
            ));
        let state = host_state(&data, cfg, fake.clone());

        block_on(create_home_record_in(&state, "小写", "/home/demo/.dsh", Some("host-lab")))
            .unwrap();
        block_on(create_home_record_in(&state, "大写", "/home/demo/.DSH", Some("host-lab")))
            .unwrap();

        assert_eq!(
            state.config.lock().unwrap().homes.len(),
            2,
            "Linux 上这是两个目录，就是两条记录"
        );

        // 本机 keeps folding case: one directory, one record.
        let scratch = data.path().join("Case");
        block_on(create_home_record_in(&state, "本机小写", &scratch.to_string_lossy(), None))
            .unwrap();
        let same = block_on(create_home_record_in(
            &state,
            "本机大写",
            &data.path().join("cASE").to_string_lossy(),
            None,
        ))
        .unwrap();
        assert_eq!(same.path, scratch, "本机不区分大小写，还是那一条");
        assert_eq!(state.config.lock().unwrap().homes.len(), 3);
    }

    /// A remote `~/.dsh` becomes a record the same way 本机's does — except
    /// that the question is put to the machine that owns the home directory.
    /// One ssh, one record, and asking twice does not add a second.
    #[test]
    fn a_remote_user_dsh_home_becomes_one_record() {
        let data = ScratchDir::new("remote-user-home");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            ..Default::default()
        };
        let answer = crate::remote::testing::marked(
            r#"{"home":"/home/demo","dsh_home":"/home/demo/.dsh","exists":true}"#,
        );
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(crate::remote::testing::FakeAnswer::stdout(answer.clone()))
            .answering(crate::remote::testing::FakeAnswer::stdout(answer));
        let state = host_state(&data, cfg, fake.clone());

        let first = block_on(list_homes_for(&state, Some("host-lab"))).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].path.to_string_lossy(), "/home/demo/.dsh");
        assert_eq!(first[0].host_id, "host-lab");
        assert_eq!(first[0].id, crate::config::user_dsh_home_id(&host));

        let second = block_on(list_homes_for(&state, Some("host-lab"))).unwrap();
        assert_eq!(second.len(), 1, "第二次问不添第二条记录");
        assert_eq!(second[0].id, first[0].id);
    }

    /// A machine without a `~/.dsh` is a machine, not an error: nothing is
    /// recorded, and the rest of its DSH_HOMEs still come back.
    #[test]
    fn a_remote_machine_without_a_user_dsh_home_records_nothing() {
        let data = ScratchDir::new("remote-no-user-home");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            homes: vec![DshHome {
                id: "h-lab".into(),
                name: "实验环境".into(),
                path: std::path::PathBuf::from("/opt/dsh/lab"),
                host_id: "host-lab".into(),
            }],
            ..Default::default()
        };
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::marked(
                r#"{"home":"/home/demo","dsh_home":"","exists":false}"#,
            )),
        );
        let state = host_state(&data, cfg, fake.clone());

        let listed = block_on(list_homes_for(&state, Some("host-lab"))).unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "h-lab");
        assert_eq!(state.config.lock().unwrap().homes.len(), 1);
    }

    /// Removing a DSH_HOME is removing a record: the directory on that machine
    /// stays, and the launcher does not even ask about it. Same rule as 本机
    /// (ADR-0006) — deleting records is the launcher's business, deleting the
    /// user's directories is not.
    #[test]
    fn removing_a_remote_home_record_touches_nothing_on_that_machine() {
        let data = ScratchDir::new("remote-home-remove");
        let host = server_host("host-lab", None);
        let cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            homes: vec![DshHome {
                id: "h-lab".into(),
                name: "实验环境".into(),
                path: std::path::PathBuf::from("/opt/dsh/lab"),
                host_id: "host-lab".into(),
            }],
            ..Default::default()
        };
        let fake = crate::remote::testing::FakeTransport::new();
        let state = host_state(&data, cfg, fake.clone());

        remove_home_record(&state, "h-lab", Some("host-lab")).unwrap();

        assert!(state.config.lock().unwrap().homes.is_empty());
        assert!(
            fake.calls().is_empty(),
            "删记录不该给那台机器发任何命令：{:?}",
            fake.calls()
        );
    }

    /// 本机's side of all of the above is unchanged: the path goes through the
    /// local rules, the directory is really created here, and nothing is dialed
    /// to answer a question about this Mac.
    #[test]
    fn the_local_host_still_creates_and_lists_its_own_homes() {
        let data = ScratchDir::new("local-home");
        let cfg = crate::config::Config {
            hosts: vec![Host::local()],
            ..Default::default()
        };
        let fake = crate::remote::testing::FakeTransport::new();
        let state = host_state(&data, cfg, fake.clone());
        let dir = data.path().join("local-dsh");

        let home = block_on(create_home_record_in(
            &state,
            "本机 HOME",
            &dir.to_string_lossy(),
            None,
        ))
        .unwrap();

        assert_eq!(home.host_id, crate::config::LOCAL_HOST_ID);
        assert!(dir.is_dir(), "本机照旧把目录建出来");
        let listed = block_on(list_homes_for(&state, None)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, home.id);
        assert!(
            fake.calls().is_empty(),
            "本机的问题不经过 ssh：{:?}",
            fake.calls()
        );
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// An `AppState` around one scratch config file, with the transport a test
    /// chose. The local lifecycle modules build theirs the same way.
    fn host_state(
        data: &ScratchDir,
        cfg: crate::config::Config,
        transport: std::sync::Arc<dyn crate::ssh::HostTransport>,
    ) -> AppState {
        AppState {
            config_path: data.path().join("config.json"),
            data_dir: data.path().to_path_buf(),
            config: std::sync::Mutex::new(cfg),
            running: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            tasks: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            host_transport: transport,
            host_statuses: std::sync::Mutex::new(std::collections::HashMap::new()),
            remote_scan: std::sync::Mutex::new(std::collections::HashSet::new()),
            forwards: Default::default(),
        }
    }

    fn host_input(name: &str, target: &str) -> HostInput {
        HostInput {
            display_name: name.into(),
            ssh_target: target.into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: None,
        }
    }

    fn remote_host(id: &str, target: &str) -> Host {
        Host {
            id: id.into(),
            display_name: "服务器".into(),
            ssh_target: target.into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: None,
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    /// The point of the whole ticket: a machine that cannot be reached is still
    /// a machine the user may keep. Saving it works, the page shows 「不可达」, and
    /// the reason is one of the four classes.
    #[test]
    fn a_host_that_cannot_be_reached_is_still_saved_and_shows_as_unreachable() {
        let data = ScratchDir::new("host-unreachable");
        let transport = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::fail(
                crate::ssh::HostErrorKind::Unreachable,
                "ssh: connect to host lab port 22: Connection refused",
            ),
        );
        let state = host_state(&data, crate::config::Config::default(), transport);

        let host = create_host_record(&state, host_input("实验室", "lab")).unwrap();

        let test = block_on(test_host_connection_for(&state, &host.id)).unwrap();
        assert!(!test.ok);
        assert_eq!(test.kind, Some(crate::ssh::HostErrorKind::Unreachable));
        assert!(test.message.unwrap().contains("Connection refused"));

        let views = list_hosts_with_status(&state).unwrap();
        let saved = views.iter().find(|v| v.host.id == host.id).unwrap();
        assert_eq!(saved.host.display_name, "实验室", "连不上的主机照样存下来");
        let status = saved.status.as_ref().unwrap();
        assert_eq!(status.state, crate::config::HostState::Unreachable);
        assert!(status.message.as_ref().unwrap().contains("Connection refused"));
        // 本机 never gets a status: it is not a machine to reach.
        let local = views.iter().find(|v| v.host.builtin).unwrap();
        assert!(local.status.is_none());
    }

    /// A machine that answered also says where its data directory is, and
    /// whether it is there yet. The launcher remembers the path so the work that
    /// follows does not have to ask again.
    #[test]
    fn a_reachable_host_remembers_where_its_data_directory_is() {
        let data = ScratchDir::new("host-reachable");
        let transport = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::connection_stdout(
                Some(("/home/linuxbrew/.linuxbrew/bin/node", "v26.0.0")),
                None,
            )),
        );
        let state = host_state(&data, crate::config::Config::default(), transport);
        let host = create_host_record(&state, host_input("实验室", "lab")).unwrap();

        let test = block_on(test_host_connection_for(&state, &host.id)).unwrap();

        assert!(test.ok);
        assert_eq!(
            test.node.as_ref().map(|t| t.version.as_str()),
            Some("v26.0.0")
        );
        assert!(test.pnpm.is_none(), "那台机器没有 pnpm，如实说没有");
        assert!(!test.data_dir_exists);

        let saved = list_hosts_with_status(&state)
            .unwrap()
            .into_iter()
            .find(|v| v.host.id == host.id)
            .unwrap();
        assert_eq!(
            saved.host.data_dir.as_deref(),
            Some(std::path::Path::new("/home/demo/.local/share/dsh-launcher")),
            "远端报出来的数据目录要记下来"
        );
        assert_eq!(
            saved.status.unwrap().state,
            crate::config::HostState::Reachable
        );
    }

    /// A data directory the user configured is the one the remote side is asked
    /// about — the launcher does not quietly replace it with its own default.
    #[test]
    fn a_configured_data_directory_is_what_the_remote_is_asked_to_check() {
        let data = ScratchDir::new("host-data-dir");
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::connection_stdout(
                Some(("/usr/bin/node", "v22.11.0")),
                Some(("/usr/bin/pnpm", "11.0.0")),
            )),
        );
        let state = host_state(&data, crate::config::Config::default(), fake.clone());
        let mut host = remote_host("host-lab", "lab");
        host.data_dir = Some(std::path::PathBuf::from("/srv/dsh launcher"));
        state.config.lock().unwrap().hosts.push(host.clone());

        let test = block_on(test_host_connection_for(&state, &host.id)).unwrap();

        assert!(test.ok);
        let script = fake.last_script();
        assert!(
            script.contains("dsh_configured='/srv/dsh launcher'"),
            "用户填的数据目录要原样问那台机器：{script}"
        );
        let saved = list_hosts_with_status(&state)
            .unwrap()
            .into_iter()
            .find(|v| v.host.id == host.id)
            .unwrap();
        assert_eq!(
            saved.host.data_dir,
            Some(std::path::PathBuf::from("/srv/dsh launcher")),
            "远端报的默认值不该覆盖用户填的"
        );
    }

    /// The form must not be able to change what a host *is*: editing 本机 leaves
    /// it 本机, with no ssh target.
    #[test]
    fn editing_a_host_keeps_its_id_and_its_builtin_flag() {
        let data = ScratchDir::new("host-edit");
        let state = host_state(
            &data,
            crate::config::Config::default(),
            crate::remote::testing::idle(),
        );

        let updated = update_host_record(
            &state,
            crate::config::LOCAL_HOST_ID,
            HostInput {
                display_name: "这台 Mac".into(),
                ssh_target: "someone@elsewhere".into(),
                ..host_input("", "")
            },
        )
        .unwrap();

        assert_eq!(updated.id, crate::config::LOCAL_HOST_ID);
        assert!(updated.builtin, "编辑不该丢掉内置标记");
        assert!(updated.ssh_target.is_empty(), "本机不经过 ssh");
        assert_eq!(updated.display_name, "这台 Mac");
    }

    /// A settings save must not wipe what the environment page owns: the probe
    /// cache and the user's hand-picked executables are not part of the host
    /// form, exactly as an instance's icon and port are not part of its form.
    #[test]
    fn editing_a_host_keeps_its_toolchain_and_its_picks() {
        let data = ScratchDir::new("host-edit-toolchain");
        let mut lab = remote_host("host-lab", "lab");
        lab.toolchain = crate::config::ToolchainBinding {
            node: Some(crate::config::BoundTool {
                path: std::path::PathBuf::from("/opt/node/bin/node"),
                version: "v22.14.0".into(),
            }),
            pnpm: None,
        };
        lab.toolchain_overrides = crate::config::ToolchainOverrides {
            node: Some(std::path::PathBuf::from("/opt/pick/node")),
            pnpm: None,
        };
        let state = host_state(
            &data,
            crate::config::Config {
                hosts: vec![Host::local(), lab],
                ..Default::default()
            },
            crate::remote::testing::idle(),
        );

        let updated = update_host_record(&state, "host-lab", host_input("实验室", "lab")).unwrap();

        assert_eq!(updated.display_name, "实验室");
        assert_eq!(
            updated.toolchain.node.as_ref().map(|bound| bound.path.clone()),
            Some(std::path::PathBuf::from("/opt/node/bin/node")),
            "探测结论不该被表单抹掉"
        );
        assert_eq!(
            updated.toolchain_overrides.node,
            Some(std::path::PathBuf::from("/opt/pick/node")),
            "用户指定的路径不该被表单抹掉"
        );
    }

    /// Deleting a host removes the launcher's records and nothing else: the
    /// other host's records stay, and 本机 cannot be deleted (ADR-0006 §5).
    #[test]
    fn deleting_a_host_drops_only_its_own_records() {
        let data = ScratchDir::new("host-delete");
        let local = Host::local();
        let lab = remote_host("host-lab", "lab");
        let mut cfg = crate::config::Config {
            hosts: vec![local.clone(), lab.clone()],
            homes: vec![
                crate::config::DshHome {
                    id: "h-local".into(),
                    name: "本机 HOME".into(),
                    path: data.path().join("local-home"),
                    host_id: local.id.clone(),
                },
                crate::config::DshHome {
                    id: "h-lab".into(),
                    name: "服务器 HOME".into(),
                    path: std::path::PathBuf::from("/home/u/.dsh"),
                    host_id: lab.id.clone(),
                },
            ],
            versions: vec![crate::config::DshVersion {
                id: "v-lab".into(),
                version: "0.1.0-rc.6".into(),
                dir: std::path::PathBuf::from(
                    "/home/u/.local/share/dsh-launcher/versions/0.1.0-rc.6",
                ),
                host_id: lab.id.clone(),
            }],
            instances: vec![
                crate::config::DshInstance {
                    id: "i-lab".into(),
                    name: "服务器实例".into(),
                    version_id: "v-lab".into(),
                    home_id: "h-lab".into(),
                    host_id: lab.id.clone(),
                    env_overrides: Default::default(),
                    default_profile: None,
                    last_profile: None,
                    icon: None,
                    port: None,
                },
                crate::config::DshInstance {
                    id: "i-local".into(),
                    name: "本机实例".into(),
                    version_id: "v-local".into(),
                    home_id: "h-local".into(),
                    host_id: local.id.clone(),
                    env_overrides: Default::default(),
                    default_profile: None,
                    last_profile: None,
                    icon: None,
                    port: None,
                },
            ],
            ..Default::default()
        };
        cfg.settings.last_instance_id = Some("i-lab".into());
        let state = host_state(&data, cfg, crate::remote::testing::idle());

        delete_host_record(&state, "host-lab").unwrap();

        let cfg = state.config.lock().unwrap();
        assert_eq!(cfg.hosts.len(), 1, "只剩本机");
        assert!(cfg.hosts[0].builtin);
        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-local"],
            "另一台主机的 HOME 记录不该被带走"
        );
        assert!(cfg.versions.is_empty(), "那台主机的版本记录跟着主机走");
        assert_eq!(
            cfg.instances.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["i-local"]
        );
        assert_eq!(
            cfg.settings.last_instance_id, None,
            "被删掉的实例不该还被记成最近用的"
        );
        drop(cfg);

        let err = delete_host_record(&state, crate::config::LOCAL_HOST_ID).unwrap_err();
        assert!(err.contains("本机"), "{err}");
    }

    /// The three refusals that keep the records honest: no name, no target, and
    /// no second host pointing at the same machine.
    #[test]
    fn a_host_needs_a_name_and_a_target_and_only_one_per_machine() {
        let data = ScratchDir::new("host-validate");
        let state = host_state(
            &data,
            crate::config::Config::default(),
            crate::remote::testing::idle(),
        );

        assert!(create_host_record(&state, host_input("  ", "lab")).is_err());
        assert!(create_host_record(&state, host_input("实验室", "  ")).is_err());

        create_host_record(&state, host_input("实验室", "lab")).unwrap();
        let again = create_host_record(&state, host_input("另一个名字", " lab ")).unwrap_err();
        assert!(again.contains("实验室"), "重复的 ssh 目标要指名道姓：{again}");
    }

    /// The built-in host is this Mac: there is nothing to connect to, and saying
    /// so is better than a dial that cannot work.
    #[test]
    fn the_local_host_has_no_connection_to_test() {
        let data = ScratchDir::new("host-local-test");
        let state = host_state(
            &data,
            crate::config::Config::default(),
            crate::remote::testing::idle(),
        );

        let err =
            block_on(test_host_connection_for(&state, crate::config::LOCAL_HOST_ID)).unwrap_err();
        assert!(err.contains("本机"), "{err}");

        let missing = block_on(test_host_connection_for(&state, "host-nope")).unwrap_err();
        assert!(missing.contains("主机不存在"), "{missing}");
    }

    #[test]
    fn remove_dedicated_home_deletes_the_dedicated_path() {
        let data = ScratchDir::new("t7-dedicated");
        let home = dedicated_home_path(data.path(), "0.1.0-rc.6");
        std::fs::create_dir_all(home.join("storages")).unwrap();
        std::fs::write(home.join("storages/session.json"), "{}").unwrap();

        remove_dedicated_home(data.path(), "0.1.0-rc.6").unwrap();

        assert!(!home.exists(), "专属 DSH_HOME 应被删除");
    }

    #[test]
    fn remove_dedicated_home_missing_path_is_not_an_error() {
        let data = ScratchDir::new("t7-missing");
        // The dedicated path was never created (or was cleaned by hand):
        // deletion must still complete without error.
        assert!(!dedicated_home_path(data.path(), "0.1.0-rc.6").exists());
        remove_dedicated_home(data.path(), "0.1.0-rc.6").unwrap();
    }

    #[test]
    fn remove_dedicated_home_leaves_other_paths_alone() {
        let data = ScratchDir::new("t7-only-by-path");
        // A stand-in for the user's `~/.dsh` (the launcher auto-adopts it and
        // an instance may point at it) and a user-specified HOME. Both are
        // DSH_HOMEs, but neither is the version's 专属 DSH_HOME.
        let user_dsh = data.path().join(".dsh");
        let user_home = data.path().join("user-specified-home");
        std::fs::create_dir_all(user_dsh.join("sessions")).unwrap();
        std::fs::create_dir_all(user_home.join("sessions")).unwrap();
        // The version whose HOME we delete...
        let target = dedicated_home_path(data.path(), "0.1.0-rc.6");
        std::fs::create_dir_all(&target).unwrap();
        // ...and a sibling version's dedicated HOME, which must also survive.
        let sibling = dedicated_home_path(data.path(), "0.1.0-rc.5");
        std::fs::create_dir_all(&sibling).unwrap();

        remove_dedicated_home(data.path(), "0.1.0-rc.6").unwrap();

        assert!(!target.exists());
        assert!(sibling.exists(), "别的版本专属 DSH_HOME 不受影响");
        assert!(user_dsh.join("sessions").is_dir(), "~/.dsh 不受影响");
        assert!(user_home.join("sessions").is_dir(), "用户指定的 HOME 不受影响");
    }

    /// The record goes with the directory, by the same path rule: `~/.dsh` and
    /// a user-specified HOME are DSH_HOMEs too and must survive — a name that
    /// merely looks like the version's is not enough. A record on another host
    /// is not this version's HOME even when the path is spelled identically.
    #[test]
    fn remove_dedicated_home_record_drops_only_that_path() {
        let data_dir = std::path::Path::new("/data/launcher");
        let local = crate::config::Host::local();
        let remote = crate::config::Host {
            id: "host-server".into(),
            display_name: "服务器".into(),
            ssh_target: "zjq_aliyun".into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: Some(std::path::PathBuf::from("/home/u/.local/share/dsh-launcher")),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        };
        let mut cfg = crate::config::Config {
            hosts: vec![local.clone(), remote.clone()],
            homes: vec![
                crate::config::DshHome {
                    id: "h-version".into(),
                    name: "0.1.0-rc.6".into(),
                    path: dedicated_home_path(data_dir, "0.1.0-rc.6"),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
                crate::config::DshHome {
                    id: "h-user-dsh".into(),
                    name: "用户默认".into(),
                    path: std::path::PathBuf::from("/Users/me/.dsh"),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
                crate::config::DshHome {
                    id: "h-sibling".into(),
                    name: "0.1.0-rc.5".into(),
                    path: dedicated_home_path(data_dir, "0.1.0-rc.5"),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
                crate::config::DshHome {
                    id: "h-remote-same-path".into(),
                    name: "0.1.0-rc.6".into(),
                    path: dedicated_home_path(data_dir, "0.1.0-rc.6"),
                    host_id: "host-server".into(),
                },
            ],
            ..Default::default()
        };

        remove_dedicated_home_record(&mut cfg, &local, data_dir, "0.1.0-rc.6");

        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-user-dsh", "h-sibling", "h-remote-same-path"],
        );
    }

    /// Deleting a version takes its dedicated DSH_HOME with it — record and
    /// directory, by the one path rule, in that order (ADR-0002). The same path
    /// on another host is that host's HOME and must survive.
    #[test]
    fn remove_version_deletes_the_dedicated_home_record_and_directory() {
        let data = ScratchDir::new("remove-version");
        let local = crate::config::Host::local();
        let remote = crate::config::Host {
            id: "host-server".into(),
            display_name: "服务器".into(),
            ssh_target: "zjq_aliyun".into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: Some(std::path::PathBuf::from("/home/u/.local/share/dsh-launcher")),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        };
        let version = "0.1.0-rc.6";
        let dedicated = dedicated_home_path(data.path(), version);
        std::fs::create_dir_all(dedicated.join("storages")).unwrap();
        let sibling = dedicated_home_path(data.path(), "0.1.0-rc.5");
        std::fs::create_dir_all(&sibling).unwrap();

        let mut cfg = crate::config::Config {
            hosts: vec![local.clone(), remote],
            versions: vec![crate::config::DshVersion {
                id: "v-rc6".into(),
                version: version.into(),
                dir: data.path().join("versions").join(version),
                host_id: crate::config::LOCAL_HOST_ID.into(),
            }],
            homes: vec![
                crate::config::DshHome {
                    id: "h-dedicated".into(),
                    name: version.into(),
                    path: dedicated.clone(),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
                crate::config::DshHome {
                    id: "h-remote-same-path".into(),
                    name: version.into(),
                    path: dedicated.clone(),
                    host_id: "host-server".into(),
                },
                crate::config::DshHome {
                    id: "h-sibling".into(),
                    name: "0.1.0-rc.5".into(),
                    path: sibling.clone(),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
            ],
            ..Default::default()
        };

        // Mirrors the body of `remove_version` without the Tauri State/app
        // plumbing: both removals use the resolved host's data directory.
        let data_dir = local.data_dir(data.path()).unwrap();
        remove_dedicated_home_record(&mut cfg, &local, &data_dir, version);
        remove_dedicated_home(&data_dir, version).unwrap();

        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-remote-same-path", "h-sibling"],
            "记录与目录一起走，别的路径与别的主机的记录都留下"
        );
        assert!(!dedicated.exists(), "专属 DSH_HOME 目录被删除");
        assert!(sibling.exists(), "别的版本专属 DSH_HOME 不受影响");
    }

    /// Deleting a version **on a remote host** takes the same three things the
    /// local delete takes — the version directory, the record, and the version's
    /// dedicated DSH_HOME (its directory and its record) — with every removal
    /// aimed at that server: a local `remove_dir_all` at a remote path would
    /// delete whatever this Mac happens to have at the same spelling.
    #[test]
    fn deleting_a_remote_version_removes_its_directory_and_home_there() {
        use crate::remote::testing::{marked, FakeAnswer, FakeCall, FakeTransport};

        let data = ScratchDir::new("remote-remove-version");
        let host = server_host("host-lab", None);
        let version = "0.1.0-rc.6";
        let remote_data = host.data_dir(data.path()).unwrap();
        let version_dir = remote_data.join("versions").join(version);
        let dedicated = host.dedicated_home_path(data.path(), version).unwrap();

        let mut cfg = crate::config::Config {
            hosts: vec![Host::local(), host.clone()],
            versions: vec![crate::config::DshVersion {
                id: "v-rc6".into(),
                version: version.into(),
                dir: version_dir.clone(),
                host_id: "host-lab".into(),
            }],
            homes: vec![
                // The dedicated HOME the delete must take with it...
                crate::config::DshHome {
                    id: "h-dedicated".into(),
                    name: version.into(),
                    path: dedicated.clone(),
                    host_id: "host-lab".into(),
                },
                // ...and a HOME of 本机 at the same spelling, which is another
                // machine's directory and stays.
                crate::config::DshHome {
                    id: "h-local-same-path".into(),
                    name: version.into(),
                    path: dedicated.clone(),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
            ],
            ..Default::default()
        };

        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(marked("{\"ok\":true}")))
            .answering(FakeAnswer::stdout(marked("{\"ok\":true}")));
        let state = host_state(&data, cfg.clone(), fake.clone());
        let data_dir = host.data_dir(&state.data_dir).unwrap();

        // The body of `remove_version`, without the Tauri State/app plumbing:
        // both removals go through the transport for a remote host.
        cfg.versions.retain(|v| v.id != "v-rc6");
        remove_dedicated_home_record(&mut cfg, &host, &data_dir, version);
        block_on(remove_remote_version_dirs(
            &state,
            &host,
            &data_dir,
            &[&version_dir, &dedicated],
        ));

        assert!(
            cfg.versions.is_empty(),
            "版本记录随它一起走"
        );
        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-local-same-path"],
            "只有那台主机上这个路径的记录被删；本机同名的路径是另一回事"
        );

        let removed: Vec<String> = fake
            .calls()
            .iter()
            .filter_map(|call| match call {
                FakeCall::Run(script) => Some(script.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(removed.len(), 2, "两个目录各删一次：{removed:?}");
        assert!(
            removed[0].contains(&format!("dsh_path='{}'", version_dir.display()))
                && removed[0].contains("rm -rf -- \"$dsh_path\""),
            "远端版本目录：{}",
            removed[0]
        );
        assert!(
            removed[1].contains(&dedicated.to_string_lossy().to_string()),
            "远端专属 DSH_HOME 目录：{}",
            removed[1]
        );        assert!(
            !data.path().join("versions").exists() && !data.path().join("homes").exists(),
            "远端路径不在本机，本机不该被碰"
        );
    }

    /// A cleanup that would reach outside the host's own data directory on that
    /// machine is refused, whatever it is spelled like: the launcher only
    /// removes what it can prove it created there (ADR-0006).
    #[test]
    fn a_remote_directory_delete_outside_the_data_directory_is_refused() {
        use crate::remote::testing::{FakeAnswer, FakeTransport};

        let data = ScratchDir::new("remote-remove-guard");
        let host = server_host("host-lab", None);
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = host_state(&data, crate::config::Config::default(), fake.clone());
        let data_dir = host.data_dir(&state.data_dir).unwrap();

        block_on(remove_remote_version_dirs(
            &state,
            &host,
            &data_dir,
            &[
                std::path::Path::new("/home/demo/.dsh"),
                &data_dir.join("../secrets"),
            ],
        ));

        assert!(
            fake.calls().is_empty(),
            "数据目录之外的路径一次都不该删：{:?}",
            fake.calls()
        );
    }

    /// An instance is a binding, not a directory: deleting one must leave its
    /// version and its DSH_HOME untouched, and must clear the "last used"
    /// pointer if it named this instance.
    #[test]
    fn remove_instance_keeps_version_and_home() {
        let data = ScratchDir::new("delete-instance");
        let home_dir = data.path().join("shared-home");
        std::fs::create_dir_all(&home_dir).unwrap();

        let mut cfg = crate::config::Config::default();
        cfg.homes.push(crate::config::DshHome {
            id: "h1".into(),
            name: "shared".into(),
            path: home_dir.clone(),
            host_id: crate::config::LOCAL_HOST_ID.into(),
        });
        cfg.versions.push(crate::config::DshVersion {
            id: "v1".into(),
            version: "0.1.0-rc.6".into(),
            dir: data.path().join("versions/0.1.0-rc.6"),
            host_id: crate::config::LOCAL_HOST_ID.into(),
        });
        cfg.instances.push(crate::config::DshInstance {
            id: "i1".into(),
            name: "one".into(),
            version_id: "v1".into(),
            home_id: "h1".into(),
            host_id: crate::config::LOCAL_HOST_ID.into(),
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        });
        cfg.settings.last_instance_id = Some("i1".into());

        // Mirrors the body of `delete_instance` without the Tauri State/app
        // plumbing: remove the record, clear the pointer, keep everything else.
        let pos = cfg.instances.iter().position(|i| i.id == "i1").unwrap();
        cfg.instances.remove(pos);
        if cfg.settings.last_instance_id.as_deref() == Some("i1") {
            cfg.settings.last_instance_id = None;
        }

        assert!(cfg.instances.is_empty(), "实例记录被删除");
        assert_eq!(cfg.versions.len(), 1, "版本不受影响");
        assert_eq!(cfg.homes.len(), 1, "DSH_HOME 不受影响");
        assert_eq!(cfg.settings.last_instance_id, None, "最近使用指针被清空");
        assert!(home_dir.is_dir(), "DSH_HOME 目录留在磁盘上");
    }

    // -----------------------------------------------------------------------
    // The machine-side actions of one instance
    // -----------------------------------------------------------------------

    /// The terminal of an instance on another machine (user story 33): one ssh
    /// command line that lands in that instance's DSH_HOME there — and nothing
    /// written on this Mac, unlike the local session's `scripts/term_*.sh`.
    #[test]
    fn a_remote_instances_terminal_opens_a_session_on_that_machine() {
        let data = ScratchDir::new("remote-terminal");
        let host = server_host("host-lab", None);
        let cfg = remote_home_cfg(&host, "/home/demo/.dsh");
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::Names(vec!["profiles".to_string()]),
        );
        let state = host_state(&data, cfg, fake.clone());

        let (inst, home) = {
            let cfg = state.config.lock().unwrap();
            (cfg.instances[0].clone(), cfg.homes[0].clone())
        };
        let session =
            block_on(remote_terminal_session(&state, &host, &inst, &home, None)).unwrap();

        // The machine is asked once, about that instance's own DSH_HOME: a host
        // that is down, or a HOME that is gone there, is a sentence in the page
        // instead of a window that opens and dies.
        assert_eq!(
            fake.calls(),
            vec![crate::remote::testing::FakeCall::ListDir(
                "/home/demo/.dsh".to_string()
            )]
        );

        assert!(
            session.line.starts_with("ssh -t zjq_aliyun "),
            "本机终端要连上那台机器：{}",
            session.line
        );
        assert!(
            session.line.contains("cd ") && session.line.contains("/home/demo/.dsh"),
            "对面那一步要进到 DSH_HOME：{}",
            session.line
        );
        assert!(
            session.line.contains("DSH_HOME") && session.line.contains("DSH_LAUNCHER_INSTANCE"),
            "会话要带上实例的变量：{}",
            session.line
        );
        assert!(
            session.label.contains("zjq") || session.label.contains("服务器"),
            "窗口标题要说清是哪台机器上的哪个实例：{}",
            session.label
        );
        assert!(
            !data.path().join("scripts").exists(),
            "远端会话不在本机写脚本文件"
        );
    }

    /// A host that is not reachable says so in the page, rather than opening a
    /// terminal that prints ssh's error and closes.
    #[test]
    fn the_machine_actions_report_an_unreachable_host() {
        use crate::remote::testing::{FakeAnswer, FakeTransport};
        let data = ScratchDir::new("remote-actions-down");
        let host = server_host("host-lab", None);
        let cfg = remote_home_cfg(&host, "/home/demo/.dsh");
        let fake = FakeTransport::new()
            .answering(FakeAnswer::fail(
                crate::ssh::HostErrorKind::Unreachable,
                "ssh: connect to host zjq_aliyun port 22: Connection refused",
            ))
            .answering(FakeAnswer::fail(
                crate::ssh::HostErrorKind::Unreachable,
                "ssh: connect to host zjq_aliyun port 22: Connection refused",
            ));
        let state = host_state(&data, cfg, fake.clone());

        let (inst, home) = {
            let cfg = state.config.lock().unwrap();
            (cfg.instances[0].clone(), cfg.homes[0].clone())
        };
        let err =
            block_on(remote_terminal_session(&state, &host, &inst, &home, None)).unwrap_err();
        assert!(err.contains("连不上"), "终端要说清是连不上：{err}");
        assert!(err.contains("服务器"), "还要说清是哪台机器：{err}");
        assert!(err.contains("Connection refused"), "原因来自 ssh：{err}");

        let err = block_on(fetch_remote_log(
            &state,
            &host,
            "/home/demo/.local/share/dsh-launcher/logs/i-lab.log",
            "i-lab",
        ))
        .unwrap_err();
        assert!(err.contains("连不上"), "日志要说清是连不上：{err}");
        assert!(err.contains("Connection refused"), "原因来自 ssh：{err}");
    }

    /// The local terminal is the file it always was (and the remote one is not
    /// a file at all): one path rule, pinned here so a remote branch cannot
    /// quietly move the local session.
    #[test]
    fn the_local_terminal_session_is_still_a_script_in_the_data_directory() {
        let data = ScratchDir::new("local-terminal-path");
        let path = local_terminal_script_path(data.path(), "i1");
        assert_eq!(path, data.path().join("scripts").join("term_i1.sh"));
    }

    /// The directory action for an instance on another machine (issue #20): the
    /// path to copy and one command that reaches it. It answers from the
    /// records, so copying works while that machine is off — there is nothing
    /// about a copy that a dead host could refuse.
    #[test]
    fn a_remote_instances_location_is_a_path_and_a_runnable_command() {
        let data = ScratchDir::new("remote-location");
        let host = server_host("host-lab", None);
        let state = host_state(
            &data,
            remote_home_cfg(&host, "/home/demo/o'brien dir"),
            crate::remote::testing::idle(),
        );

        let location = instance_location_for(&state, "i-lab").unwrap();

        assert_eq!(location.path, "/home/demo/o'brien dir");
        assert_eq!(location.host_name, "服务器");
        assert_eq!(location.ssh_target, "zjq_aliyun");
        assert!(
            location.command.starts_with("ssh zjq_aliyun "),
            "命令要能在本机终端里跑：{}",
            location.command
        );
        assert!(
            location.command.contains("ls -la"),
            "命令要在那台机器上看那个目录：{}",
            location.command
        );

        // The path holds a single quote, so the proof that it survived is the
        // shape the user's own shell parses: three words, then no more.
        let handed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s@@@' {}", location.command))
            .output()
            .unwrap();
        let handed_text = String::from_utf8_lossy(&handed.stdout).to_string();
        let words: Vec<&str> = handed_text.split("@@@").collect();
        assert_eq!(
            words.len(),
            4,
            "ssh 只该拿到一个参数：{words:?}（路径把它拆开了）"
        );
        assert_eq!(words[0], "ssh");
        assert_eq!(words[1], "zjq_aliyun");
    }

    /// A local instance keeps 「打开目录」: the copy actions are the remote
    /// answer to a directory this Mac cannot open, not a replacement for the
    /// one it can.
    #[test]
    fn a_local_instances_directory_is_still_the_one_button_it_was() {
        let data = ScratchDir::new("local-location");
        let cfg = crate::config::Config {
            homes: vec![DshHome {
                id: "h1".into(),
                name: "默认".into(),
                path: data.path().join("home"),
                host_id: crate::config::LOCAL_HOST_ID.into(),
            }],
            instances: vec![instance_record("i1", "本机实例", crate::config::LOCAL_HOST_ID)],
            ..Default::default()
        };
        let state = host_state(&data, cfg, crate::remote::testing::idle());

        let err = instance_location_for(&state, "i1").unwrap_err();

        assert!(err.contains("打开目录"), "{err}");
    }

    /// An instance's log: 本机's is read where it is written, and another
    /// machine's is copied home — the last stretch of it, into the launcher's
    /// own log directory, so the file manager can reveal it exactly as it
    /// reveals a local one (user story 34).
    #[test]
    fn a_remote_instances_log_is_copied_home_and_a_local_one_is_read_in_place() {
        let data = ScratchDir::new("remote-log");
        let host = server_host("host-lab", None);
        let mut cfg = remote_home_cfg(&host, "/home/demo/.dsh");
        cfg.homes.push(DshHome {
            id: "h1".into(),
            name: "默认".into(),
            path: data.path().join("home"),
            host_id: crate::config::LOCAL_HOST_ID.into(),
        });
        cfg.instances
            .push(instance_record("i1", "本机实例", crate::config::LOCAL_HOST_ID));
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(remote_stdout(r#"{"exists":true,"data":"dGFpbAo="}"#));
        let state = host_state(&data, cfg, fake.clone());

        let local = block_on(instance_log_source(&state, "i1")).unwrap();
        match local {
            InstanceLog::Local(path) => {
                assert_eq!(path, data.path().join("logs").join("i1.log"));
            }
            InstanceLog::Remote { .. } => panic!("本机实例的日志不在远端"),
        }

        let remote_log = match block_on(instance_log_source(&state, "i-lab")).unwrap() {
            InstanceLog::Remote { log_path, .. } => log_path,
            InstanceLog::Local(_) => panic!("远端实例的日志在那台机器上"),
        };
        assert_eq!(
            remote_log,
            "/home/demo/.local/share/dsh-launcher/logs/i-lab.log",
            "读的就是那台机器上那个实例的日志"
        );

        let copied = block_on(fetch_remote_log(
            &state,
            &host,
            &remote_log,
            "i-lab",
        ))
        .unwrap();
        assert_eq!(copied, data.path().join("logs").join("i-lab.remote.log"));
        assert_eq!(std::fs::read_to_string(&copied).unwrap(), "tail\n");
        let script = fake.last_script();
        assert!(script.contains(&crate::remote::quote(&remote_log)), "{script}");
        assert!(
            script.contains(&format!("dsh_bytes={}", crate::remote::LOG_TAIL_BYTES)),
            "只拉最后一段，不是整个文件：{script}"
        );
    }
}
