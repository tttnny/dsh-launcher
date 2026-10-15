use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Persistent configuration models
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshHome {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// The host this DSH_HOME lives on. Empty in a config written before hosts
    /// existed; [`migrate_hosts`] fills it in at load time.
    #[serde(default)]
    pub host_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshVersion {
    pub id: String,
    pub version: String,
    pub dir: PathBuf,
    /// The host this version is installed on. Empty in a config written before
    /// hosts existed; [`migrate_hosts`] fills it in at load time.
    #[serde(default)]
    pub host_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshInstance {
    pub id: String,
    pub name: String,
    pub version_id: String,
    pub home_id: String,
    /// The host this instance runs on. Empty in a config written before hosts
    /// existed; [`migrate_hosts`] fills it in at load time.
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub env_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub last_profile: Option<String>,
    /// Instance icon: an http(s) URL, or "local" for a cropped PNG stored at
    /// `<home>/icons/<id>.png`. `None` falls back to the launcher icon.
    #[serde(default)]
    pub icon: Option<String>,
    /// Preferred web port (issue #21): `Some(1-65535)` pins it; `None` binds
    /// a random free port (`--port 0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// The built-in local host's id. A literal rather than [`new_id`]: migrations,
/// tests and the frontend need one fixed reference to 本机, and a generated one
/// would differ on every machine.
pub const LOCAL_HOST_ID: &str = "host-local";

/// One machine the launcher manages. The local host is the built-in one; every
/// other host is reached over ssh. A host owns its versions, DSH_HOMEs,
/// profiles and instances, and knows where its own data directory is —
/// because those are paths on *that* machine's filesystem and processes in
/// *its* process table, not entries in the launcher's records (ADR-0006).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    /// The name the UI shows. The built-in host's is 本机.
    pub display_name: String,
    /// The ssh target handed to the system `ssh` as it is typed: an alias from
    /// `~/.ssh/config`, or a free string such as `user@host -p 2222`. Empty for
    /// the local host, which nothing is dialed for.
    #[serde(default)]
    pub ssh_target: String,
    /// The npm registry this host's versions are listed from, when it has its
    /// own. `None` = the launcher's registry.
    #[serde(default)]
    pub registry: Option<String>,
    /// Environment variables injected into everything the launcher runs on this
    /// host, installs and instances alike.
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
    /// The launcher's data directory *on this host*: installed versions,
    /// dedicated DSH_HOMEs, logs, the pnpm store and the running record all
    /// live under it. `None` for the local host — that is the launcher's own
    /// data directory — and for a remote host whose default has not been read
    /// off that machine yet.
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    /// Built-in hosts cannot be deleted. True for 本机 only, and what the
    /// settings page reads to leave out the delete control.
    #[serde(default)]
    pub builtin: bool,
    /// Where this host's toolchain resolution last landed: the absolute paths of
    /// the `node` and `pnpm` the launcher will drive DSH with *there*. A probe
    /// cache — rewritten by every probe, never a user preference — and one per
    /// host, because a path is a fact about one machine's filesystem (ADR-0005
    /// 追加).
    #[serde(default)]
    pub toolchain: ToolchainBinding,
    /// Executables the user picked by hand for *this* host, on the environment
    /// page. A chosen path wins over detection there, and only the override
    /// command writes it.
    #[serde(default)]
    pub toolchain_overrides: ToolchainOverrides,
}

impl Host {
    /// The built-in local host: this Mac, the machine the launcher itself runs
    /// on.
    pub fn local() -> Self {
        Self {
            id: LOCAL_HOST_ID.to_string(),
            display_name: "本机".to_string(),
            ssh_target: String::new(),
            registry: None,
            extra_env: BTreeMap::new(),
            data_dir: None,
            builtin: true,
            toolchain: ToolchainBinding::default(),
            toolchain_overrides: ToolchainOverrides::default(),
        }
    }

    /// Where this host keeps the launcher's own data (installed versions,
    /// dedicated DSH_HOMEs, logs, the pnpm store, its running record).
    ///
    /// The local host's is the launcher's own data directory. A remote host
    /// states its own, because its default (that machine's home directory plus
    /// `~/.local/share/dsh-launcher`) can only be resolved on that machine.
    pub fn data_dir(&self, launcher_data_dir: &Path) -> Result<PathBuf, String> {
        if self.builtin {
            return Ok(launcher_data_dir.to_path_buf());
        }
        self.data_dir
            .clone()
            .ok_or_else(|| format!("主机「{}」还没有远端数据目录", self.display_name))
    }

    /// This host's dedicated DSH_HOME for a version:
    /// `<该主机的数据目录>/homes/<版本名>` (ADR-0002).
    pub fn dedicated_home_path(
        &self,
        launcher_data_dir: &Path,
        version: &str,
    ) -> Result<PathBuf, String> {
        Ok(dedicated_home_path(
            &self.data_dir(launcher_data_dir)?,
            version,
        ))
    }

    /// Path equality *on this host*: the local APFS volume folds case, a remote
    /// Linux filesystem does not. [`paths_equal`] is the local half of this
    /// rule, for the call sites that have no host in hand yet.
    pub fn paths_equal(&self, a: &Path, b: &Path) -> bool {
        if self.builtin {
            paths_equal(a, b)
        } else {
            a == b
        }
    }
}

/// What the launcher last learned about reaching one host: 连接中 while a test
/// is in flight, then 可达 or 不可达.
///
/// This is **runtime**, never a field of `config.json`: 「现在连不上」 is a fact
/// about the network and the moment, not about the host record. A machine that
/// is off is still a machine the user saved (ADR-0006).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    Connecting,
    Reachable,
    Unreachable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostStatus {
    pub state: HostState,
    /// Why, when `Unreachable`: the readable sentence of one of the four
    /// failure classes.
    #[serde(default)]
    pub message: Option<String>,
}

impl HostStatus {
    pub fn connecting() -> Self {
        Self {
            state: HostState::Connecting,
            message: None,
        }
    }

    pub fn reachable() -> Self {
        Self {
            state: HostState::Reachable,
            message: None,
        }
    }

    pub fn unreachable(message: impl Into<String>) -> Self {
        Self {
            state: HostState::Unreachable,
            message: Some(message.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LauncherSettings {
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub last_instance_id: Option<String>,
    /// UI theme: "light" | "dark" | "system" (follow the OS setting).
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Runtime log level: "debug" | "info" | "warn" | "error".
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Route the launcher's own HTTP requests through a proxy.
    #[serde(default)]
    pub proxy_enabled: bool,
    /// Proxy URL without port, e.g. `http://127.0.0.1`.
    #[serde(default = "default_proxy_url")]
    pub proxy_url: String,
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
    /// Comma-separated hosts that bypass the proxy (NO_PROXY).
    #[serde(default = "default_no_proxy")]
    pub no_proxy: String,
    /// Also inject the proxy into launched dsh instances, overriding the
    /// instance's own environment variables (applies on next start).
    #[serde(default)]
    pub proxy_apply_dsh: bool,
    /// External terminal for instance shells: "system" (Terminal.app) or
    /// "ghostty". The embedded PTY is gone; the launcher opens a real
    /// terminal window with DSH_HOME set and cwd at the HOME directory.
    #[serde(default = "default_terminal")]
    pub terminal: String,
}

/// The user's hand-picked `node` / `pnpm` executables. Each is `None` until the
/// user points the setup page at one.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolchainOverrides {
    #[serde(default)]
    pub node: Option<PathBuf>,
    #[serde(default)]
    pub pnpm: Option<PathBuf>,
}

/// The resolved toolchain. Each entry is `None` until a probe finds it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolchainBinding {
    #[serde(default)]
    pub node: Option<BoundTool>,
    #[serde(default)]
    pub pnpm: Option<BoundTool>,
}

/// One bound tool: the absolute path the launcher will execute, plus the
/// version it reported when it was probed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundTool {
    pub path: PathBuf,
    pub version: String,
}

impl ToolchainBinding {
    /// Whether a bound tool is still usable as recorded. A path that vanished
    /// (a deleted nvm version, an uninstalled brew formula) makes the binding
    /// stale, which the resolver treats as "probe again".
    ///
    /// A tool the user pointed at is fresh only while the binding is still that
    /// very file: any other recorded path means the choice has not been applied
    /// yet, so the resolver has to run again.
    fn is_fresh(&self, tool: &Option<BoundTool>, manual: Option<&Path>) -> bool {
        match tool {
            Some(bound) => match manual {
                Some(manual) => paths_equal(&bound.path, manual) && manual.is_file(),
                None => bound.path.is_file(),
            },
            None => false,
        }
    }

    pub fn node_is_fresh(&self, manual: Option<&Path>) -> bool {
        self.is_fresh(&self.node, manual)
    }

    pub fn pnpm_is_fresh(&self, manual: Option<&Path>) -> bool {
        self.is_fresh(&self.pnpm, manual)
    }
}

fn default_terminal() -> String {
    "system".to_string()
}

fn default_locale() -> String {
    "zh-CN".to_string()
}

fn default_true() -> bool {
    true
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_proxy_url() -> String {
    "http://127.0.0.1".to_string()
}

fn default_proxy_port() -> u16 {
    7890
}

fn default_no_proxy() -> String {
    "127.0.0.1,localhost,::1".to_string()
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            locale: default_locale(),
            minimize_to_tray: default_true(),
            autostart: false,
            last_instance_id: None,
            theme: default_theme(),
            log_level: default_log_level(),
            proxy_enabled: false,
            proxy_url: default_proxy_url(),
            proxy_port: default_proxy_port(),
            no_proxy: default_no_proxy(),
            proxy_apply_dsh: false,
            terminal: default_terminal(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub homes: Vec<DshHome>,
    #[serde(default)]
    pub versions: Vec<DshVersion>,
    #[serde(default)]
    pub instances: Vec<DshInstance>,
    #[serde(default)]
    pub settings: LauncherSettings,
    /// The machines the launcher manages. The built-in local host is always in
    /// here — a config written before hosts existed gains it on load.
    #[serde(default)]
    pub hosts: Vec<Host>,
}

impl Default for Config {
    /// A launcher that has never written a config still has one host: the
    /// machine it is running on.
    fn default() -> Self {
        Self {
            homes: Vec::new(),
            versions: Vec::new(),
            instances: Vec::new(),
            settings: LauncherSettings::default(),
            hosts: vec![Host::local()],
        }
    }
}

impl Config {
    /// The built-in local host, when the config carries one.
    pub fn local_host(&self) -> Option<&Host> {
        self.hosts.iter().find(|h| h.builtin)
    }
}

/// The host a command acts on: the one the caller named, or 本机 when it named
/// none — which is what every call site written before hosts existed does.
pub fn resolve_host<'a>(cfg: &'a Config, host_id: Option<&str>) -> Result<&'a Host, String> {
    match host_id.map(str::trim).filter(|id| !id.is_empty()) {
        None => cfg.local_host().ok_or_else(|| "配置里没有本机记录".to_string()),
        Some(id) => cfg
            .hosts
            .iter()
            .find(|h| h.id == id)
            .ok_or_else(|| format!("主机不存在: {id}")),
    }
}

/// Gives every record written before hosts existed the local host, and makes
/// sure the built-in host is in the list.
///
/// This runs before the other migrations: they merge and sweep HOME records by
/// path, and a path only identifies one record within the host that owns it.
pub fn migrate_hosts(cfg: &mut Config) -> bool {
    let mut changed = false;

    // The built-in host is the one the frontend reads (`builtin`); the literal
    // id is what migrations and tests reference.
    if !cfg.hosts.iter().any(|h| h.builtin) {
        match cfg.hosts.iter_mut().find(|h| h.id == LOCAL_HOST_ID) {
            Some(local) => local.builtin = true,
            None => cfg.hosts.push(Host::local()),
        }
        changed = true;
    }

    let local_id = LOCAL_HOST_ID;
    for home in &mut cfg.homes {
        if home.host_id.is_empty() {
            home.host_id = local_id.to_string();
            changed = true;
        }
    }
    for version in &mut cfg.versions {
        if version.host_id.is_empty() {
            version.host_id = local_id.to_string();
            changed = true;
        }
    }
    for instance in &mut cfg.instances {
        if instance.host_id.is_empty() {
            instance.host_id = local_id.to_string();
            changed = true;
        }
    }

    changed
}

// ---------------------------------------------------------------------------
// API / event payloads (mirrored by the frontend)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteVersion {
    pub version: String,
    pub released_at: Option<String>,
    /// Where the version comes from: absent/`npm` installs from the registry;
    /// `github` marks a GitHub-only tag (dsh-v*) that must be built from
    /// source (clone + pnpm install + build).
    #[serde(default)]
    pub source: Option<String>,
}

/// Partial settings update: only present fields are applied.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SettingsPatch {
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub minimize_to_tray: Option<bool>,
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub last_instance_id: Option<String>,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub proxy_enabled: Option<bool>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub proxy_port: Option<u16>,
    #[serde(default)]
    pub no_proxy: Option<String>,
    #[serde(default)]
    pub proxy_apply_dsh: Option<bool>,
    #[serde(default)]
    pub terminal: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstanceState {
    Stopped,
    Starting,
    Running,
    Exited,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub id: String,
    pub state: InstanceState,
    pub url: Option<String>,
    pub profile: Option<String>,
    pub exit_code: Option<i32>,
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

pub fn load_config(path: &Path) -> Config {
    let Ok(raw) = fs::read_to_string(path) else {
        return Config::default();
    };
    // The document is read as a value before it is read as a `Config`: a config
    // written before each host kept its own toolchain still spells that pair
    // under `settings`, and only the raw document can say what it was.
    let Ok(document) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return backup_and_reset(path, "配置不是合法的 JSON");
    };
    match serde_json::from_value::<Config>(document.clone()) {
        Ok(mut cfg) => {
            let data_dir = path.parent().unwrap_or(Path::new("."));
            // Ownership first: the sweeps below compare HOME paths, which
            // only identify one record per host.
            let hosts_changed = migrate_hosts(&mut cfg);
            let adopted = adopt_legacy_toolchain(&document, &mut cfg);
            let local = cfg.local_host().cloned().unwrap_or_else(Host::local);
            dedupe_homes(&local, &mut cfg);
            cleanup_orphan_homes(&local, &mut cfg);
            ensure_user_dsh_home(&local, &mut cfg);
            let changed = hosts_changed
                || adopted
                || migrate_instances_to_dedicated_homes(&local, data_dir, &mut cfg);
            if changed {
                let _ = save_config(path, &cfg);
            }
            cfg
        }
        Err(err) => backup_and_reset(path, &err.to_string()),
    }
}

/// Keeps the broken file and starts fresh.
fn backup_and_reset(path: &Path, reason: &str) -> Config {
    let _ = fs::copy(path, path.with_extension("json.bak"));
    eprintln!("dsh-launcher: config corrupted, backed up: {reason}");
    Config::default()
}

/// Moves the toolchain pair a config written before hosts owned it kept under
/// `settings` onto the local host, which is whose pair it was.
///
/// Without this the user's hand-picked executables would be dropped: the pair is
/// per host now, and an unknown key in the document is simply not read.
fn adopt_legacy_toolchain(document: &serde_json::Value, cfg: &mut Config) -> bool {
    let settings = &document["settings"];
    let binding: ToolchainBinding =
        serde_json::from_value(settings["toolchain"].clone()).unwrap_or_default();
    let overrides: ToolchainOverrides =
        serde_json::from_value(settings["toolchain_overrides"].clone()).unwrap_or_default();
    let legacy_is_empty = binding.node.is_none()
        && binding.pnpm.is_none()
        && overrides.node.is_none()
        && overrides.pnpm.is_none();
    if legacy_is_empty {
        return false;
    }
    let Some(local) = cfg.hosts.iter_mut().find(|host| host.builtin) else {
        return false;
    };
    let local_is_empty = local.toolchain.node.is_none()
        && local.toolchain.pnpm.is_none()
        && local.toolchain_overrides.node.is_none()
        && local.toolchain_overrides.pnpm.is_none();
    if !local_is_empty {
        return false;
    }
    local.toolchain = binding;
    local.toolchain_overrides = overrides;
    true
}

/// Removes one host's HOME records whose directory no longer exists AND that
/// are not referenced by any instance (a stale placeholder from an interrupted
/// task or a manually deleted folder). Directories that still exist are kept
/// even if unreferenced (they may be user-managed).
///
/// Only `host`'s records are judged: "the directory does not exist" is a
/// statement about *that* machine's filesystem, and the launcher can only look
/// at its own.
pub fn cleanup_orphan_homes(host: &Host, cfg: &mut Config) {
    let orphans: Vec<String> = cfg
        .homes
        .iter()
        .filter(|h| h.host_id == host.id)
        .filter(|h| !h.path.exists() && !cfg.instances.iter().any(|i| i.home_id == h.id))
        .map(|h| h.id.clone())
        .collect();
    if orphans.is_empty() {
        return;
    }
    cfg.homes.retain(|h| !orphans.contains(&h.id));
}

/// If the user's home directory contains a `.dsh` folder, make sure a HOME
/// record points at it so it can be picked as a DSH_HOME and referenced by
/// instances. Idempotent.
pub fn ensure_user_dsh_home(host: &Host, cfg: &mut Config) {
    // `~` here is expanded against the launcher's own HOME, so this question
    // can only be asked of the local host.
    if !host.builtin {
        return;
    }
    let home_dir = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .map(std::path::PathBuf::from);
    let Some(home_dir) = home_dir else { return };
    let dsh = home_dir.join(".dsh");
    if !dsh.exists() {
        return;
    }
    if cfg
        .homes
        .iter()
        .any(|h| h.host_id == host.id && host.paths_equal(&h.path, &dsh))
    {
        return;
    }
    cfg.homes.push(DshHome {
        id: "home-user-dsh".to_string(),
        name: "用户默认 (~/.dsh)".to_string(),
        path: dsh,
        host_id: host.id.clone(),
    });
}

/// Path equality. APFS volumes are case-insensitive by default, so the
/// comparison folds case on macOS (matching how the filesystem itself
/// resolves paths); elsewhere plain equality is used. This is the local host's
/// rule — see [`Host::paths_equal`] for the per-host one.
pub fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }
    #[cfg(not(target_os = "macos"))]
    {
        a == b
    }
}

/// The dedicated DSH_HOME of a version under one data directory:
/// `<数据目录>/homes/<版本名>`.
///
/// One version, one directory — the contract the installer enforces
/// (`migrate_instances_to_dedicated_homes`, `start_create_instance_task`). The
/// derivation must stay identical to theirs (including [`sanitize_name`]) so
/// that the path a deletion removes is provably the path the launcher created,
/// and can never resolve to `~/.dsh` or a user-specified HOME (ADR-0002).
pub(crate) fn dedicated_home_path(data_dir: &Path, version: &str) -> PathBuf {
    data_dir.join("homes").join(sanitize_name(version))
}

/// Merge one host's HOME records that point at the same path, keeping the
/// first and redirecting instance references to the surviving home id (cleans
/// up duplicates created before path-based reuse existed).
///
/// Two hosts may both have a HOME at `/home/me/.dsh`: same spelling, two
/// machines, two records.
pub fn dedupe_homes(host: &Host, cfg: &mut Config) {
    let mut redirect: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    {
        let mut kept: Vec<&DshHome> = Vec::new();
        for home in cfg.homes.iter().filter(|h| h.host_id == host.id) {
            if let Some(existing) = kept
                .iter()
                .find(|e| host.paths_equal(&e.path, &home.path))
            {
                redirect.insert(home.id.clone(), existing.id.clone());
            } else {
                kept.push(home);
            }
        }
    }
    if redirect.is_empty() {
        return;
    }
    for inst in &mut cfg.instances {
        if let Some(new_id) = redirect.get(&inst.home_id) {
            inst.home_id = new_id.clone();
        }
    }
    cfg.homes.retain(|h| !redirect.contains_key(&h.id));
}

/// Enforces the 1-version-1-instance contract and migrates `host`'s instances
/// to dedicated homes under `<data_dir>/homes/<version>` (issue #3, #4), where
/// `data_dir` is that host's data directory.
pub fn migrate_instances_to_dedicated_homes(host: &Host, data_dir: &Path, cfg: &mut Config) -> bool {
    let mut modified = false;

    // 1. Remove instances whose version no longer exists.
    let old_inst_len = cfg.instances.len();
    cfg.instances.retain(|i| cfg.versions.iter().any(|v| v.id == i.version_id));
    if cfg.instances.len() != old_inst_len {
        modified = true;
    }

    // 2. Ensure each version has its dedicated home and 1:1 instance.
    for v in cfg.versions.iter().filter(|v| v.host_id == host.id) {
        let expected_home_path = dedicated_home_path(data_dir, &v.version);

        // Find existing home by path or create it.
        let home_id = if let Some(h) = cfg
            .homes
            .iter()
            .find(|h| h.host_id == host.id && host.paths_equal(&h.path, &expected_home_path))
        {
            h.id.clone()
        } else {
            let hid = new_id("h");
            cfg.homes.push(DshHome {
                id: hid.clone(),
                name: v.version.clone(),
                path: expected_home_path.clone(),
                host_id: host.id.clone(),
            });
            modified = true;
            hid
        };

        // Find existing instance for this version.
        if let Some(inst) = cfg.instances.iter_mut().find(|i| i.version_id == v.id) {
            if inst.name != v.version {
                inst.name = v.version.clone();
                modified = true;
            }
            // If the instance's home_id is missing or invalid, point it to its dedicated home.
            if !cfg.homes.iter().any(|h| h.id == inst.home_id) {
                inst.home_id = home_id;
                modified = true;
            }
        } else {
            cfg.instances.push(DshInstance {
                id: new_id("i"),
                name: v.version.clone(),
                version_id: v.id.clone(),
                home_id,
                host_id: host.id.clone(),
                env_overrides: BTreeMap::new(),
                default_profile: Some("web".to_string()),
                last_profile: None,
                icon: None,
                port: None,
            });
            modified = true;
        }
    }

    modified
}

pub fn save_config(path: &Path, cfg: &Config) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, raw).map_err(|e| format!("写入配置失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("保存配置失败: {e}"))?;
    Ok(())
}

pub fn sanitize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '.' || ch == ' ' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        "instance".to_string()
    } else {
        trimmed
    }
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

/// Normalizes a user-entered directory path into an absolute one.
///
/// The launcher's path inputs are free-text: without this, `~/.dsh` was taken
/// literally and `create_dir_all` built a directory named `~` under whatever
/// cwd the app happened to have. Rules, applied to every such input:
///
/// * a leading `~` (alone, `~/…`, or `~user/…`) expands to `$HOME`;
/// * `.` / `..` segments are resolved textually;
/// * trailing separators are dropped (`/a/b/` and `/a/b` are one HOME);
/// * a relative path is rejected — its meaning would depend on the app's cwd.
///
/// The result is not canonicalized: symlinks are intentionally preserved so a
/// user-managed `/Volumes/data/dsh` stays spelled that way.
pub fn normalize_dir_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空".to_string());
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let expanded: PathBuf = if trimmed == "~" {
        home.clone().ok_or_else(|| "无法解析 ~：未设置 HOME".to_string())?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.clone()
            .ok_or_else(|| "无法解析 ~：未设置 HOME".to_string())?
            .join(rest)
    } else if trimmed.starts_with('~') {
        // `~user/...` needs the user database; treat it as unsupported rather
        // than silently creating a literal `~user` directory.
        return Err(format!("不支持 ~用户名 形式的路径：{trimmed}"));
    } else {
        PathBuf::from(trimmed)
    };

    // Resolve `.`/`..` and drop trailing separators without touching the
    // filesystem (the path may not exist yet).
    let mut out = PathBuf::new();
    for comp in expanded.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return Err(format!("路径越界：{trimmed}"));
                }
            }
            other => out.push(other.as_os_str()),
        }
    }

    if !out.is_absolute() {
        return Err(format!("请填写绝对路径（以 / 开头）：{trimmed}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests need a stable HOME; the expansion target only has to be
    /// absolute and match whatever HOME says.
    fn home() -> PathBuf {
        std::env::var_os("HOME").map(PathBuf::from).unwrap()
    }

    /// A remote host with its own data directory (or none yet).
    fn remote_host(id: &str, data_dir: Option<&str>) -> Host {
        Host {
            id: id.to_string(),
            display_name: format!("远端 {id}"),
            ssh_target: "zjq_aliyun".to_string(),
            registry: None,
            extra_env: BTreeMap::new(),
            data_dir: data_dir.map(PathBuf::from),
            builtin: false,
            toolchain: ToolchainBinding::default(),
            toolchain_overrides: ToolchainOverrides::default(),
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsh-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The config this launcher wrote before hosts existed, byte for byte: one
    /// version, its dedicated DSH_HOME, one instance and a pile of settings.
    const OLD_CONFIG_JSON: &str = r#"{
  "homes": [
    {
      "id": "h-dedicated",
      "name": "0.1.0-rc.6",
      "path": "/data/launcher/homes/0.1.0-rc.6"
    }
  ],
  "versions": [
    {
      "id": "v-rc6",
      "version": "0.1.0-rc.6",
      "dir": "/data/launcher/versions/0.1.0-rc.6"
    }
  ],
  "instances": [
    {
      "id": "i-web",
      "name": "0.1.0-rc.6",
      "version_id": "v-rc6",
      "home_id": "h-dedicated",
      "env_overrides": { "DSH_TELEMETRY_DISABLED": "1" },
      "default_profile": "web",
      "last_profile": "demo",
      "icon": "https://example.com/icon.png",
      "port": 34567
    }
  ],
  "settings": {
    "locale": "en-US",
    "minimize_to_tray": false,
    "autostart": true,
    "last_instance_id": "i-web",
    "theme": "dark",
    "log_level": "debug",
    "proxy_enabled": true,
    "proxy_url": "http://127.0.0.1",
    "proxy_port": 7890,
    "no_proxy": "127.0.0.1",
    "proxy_apply_dsh": true,
    "terminal": "ghostty",
    "toolchain": { "node": { "path": "/opt/node/bin/node", "version": "v22.0.0" } },
    "toolchain_overrides": { "pnpm": "/opt/pnpm/bin/pnpm" }
  }
}"#;

    /// Opening a config that predates hosts gives every record the local host
    /// and adds the built-in one — without touching anything else in it.
    #[test]
    fn old_config_records_join_the_local_host() {
        let dir = scratch("hosts-migrate");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, OLD_CONFIG_JSON).unwrap();

        let cfg = load_config(&config_path);

        // The built-in host is there, and it is the only one.
        assert_eq!(cfg.hosts.len(), 1, "只有本机一条");
        let local = cfg.local_host().expect("本机必须在");
        assert_eq!(local.id, LOCAL_HOST_ID);
        assert!(local.builtin, "本机是内置的那一条，不能删除");
        assert_eq!(local.display_name, "本机");

        // Every record now has an owner...
        for h in &cfg.homes {
            assert_eq!(h.host_id, LOCAL_HOST_ID, "DSH_HOME {} 归本机", h.id);
        }
        for v in &cfg.versions {
            assert_eq!(v.host_id, LOCAL_HOST_ID, "版本 {} 归本机", v.id);
        }
        for i in &cfg.instances {
            assert_eq!(i.host_id, LOCAL_HOST_ID, "实例 {} 归本机", i.id);
        }

        // ...and nothing else moved.
        let home = cfg.homes.iter().find(|h| h.id == "h-dedicated").unwrap();
        assert_eq!(home.name, "0.1.0-rc.6");
        assert_eq!(home.path, PathBuf::from("/data/launcher/homes/0.1.0-rc.6"));
        let version = cfg.versions.iter().find(|v| v.id == "v-rc6").unwrap();
        assert_eq!(version.version, "0.1.0-rc.6");
        assert_eq!(version.dir, PathBuf::from("/data/launcher/versions/0.1.0-rc.6"));
        let inst = cfg.instances.iter().find(|i| i.id == "i-web").unwrap();
        assert_eq!(inst.version_id, "v-rc6");
        assert_eq!(inst.home_id, "h-dedicated");
        assert_eq!(
            inst.env_overrides.get("DSH_TELEMETRY_DISABLED"),
            Some(&"1".to_string())
        );
        assert_eq!(inst.default_profile.as_deref(), Some("web"));
        assert_eq!(inst.last_profile.as_deref(), Some("demo"));
        assert_eq!(inst.icon.as_deref(), Some("https://example.com/icon.png"));
        assert_eq!(inst.port, Some(34567));
        assert_eq!(cfg.settings.locale, "en-US");
        assert_eq!(cfg.settings.theme, "dark");
        assert_eq!(cfg.settings.log_level, "debug");
        assert!(cfg.settings.proxy_enabled);
        assert!(cfg.settings.proxy_apply_dsh);
        assert!(!cfg.settings.minimize_to_tray);
        assert!(cfg.settings.autostart);
        assert_eq!(cfg.settings.terminal, "ghostty");

        // The toolchain pair this config spells under `settings` belonged to the
        // local host all along; it moves onto that host's record, so the user's
        // hand-picked executables are not dropped by the move.
        assert_eq!(
            local.toolchain.node.as_ref().map(|b| b.path.clone()),
            Some(PathBuf::from("/opt/node/bin/node"))
        );
        assert_eq!(
            local.toolchain_overrides.pnpm,
            Some(PathBuf::from("/opt/pnpm/bin/pnpm"))
        );
        assert_eq!(cfg.settings.last_instance_id.as_deref(), Some("i-web"));

        // The migration is persisted, so the next launch starts from a config
        // that already names the host.
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(written["hosts"][0]["id"], LOCAL_HOST_ID);
        assert_eq!(written["instances"][0]["host_id"], LOCAL_HOST_ID);
        assert_eq!(
            written["hosts"][0]["toolchain"]["node"]["path"],
            "/opt/node/bin/node"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A launcher that has never written a config is still on one host.
    #[test]
    fn a_fresh_config_already_has_the_local_host() {
        let cfg = Config::default();
        let local = cfg.local_host().expect("本机必须在");
        assert_eq!(local.id, LOCAL_HOST_ID);
        assert!(local.builtin);
        assert!(resolve_host(&cfg, None).is_ok(), "不指定主机时解析到本机");
    }

    /// A config file that has no `hosts` key at all (every file written before
    /// this change) still comes back with the built-in host, and naming it by
    /// its literal id works.
    #[test]
    fn a_config_without_a_hosts_key_still_resolves_the_local_host() {
        let dir = scratch("hosts-missing-key");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, r#"{"settings":{"locale":"zh-CN"}}"#).unwrap();

        let cfg = load_config(&config_path);

        assert_eq!(cfg.hosts.len(), 1);
        assert!(resolve_host(&cfg, Some(LOCAL_HOST_ID)).is_ok());
        let err = resolve_host(&cfg, Some("host-nope")).unwrap_err();
        assert!(err.contains("主机不存在"), "err: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The dedicated DSH_HOME is `<该主机的数据目录>/homes/<版本名>`: the same
    /// rule as before, applied to the host that owns the version.
    #[test]
    fn dedicated_home_paths_are_computed_on_their_host() {
        let local = Host::local();
        assert_eq!(
            local
                .dedicated_home_path(Path::new("/data/launcher"), "0.1.0-rc.6")
                .unwrap(),
            Path::new("/data/launcher/homes/0.1.0-rc.6"),
        );
        // Sanitized with the same rule the installer uses.
        assert_eq!(
            local
                .dedicated_home_path(Path::new("/data/launcher"), "a/b c")
                .unwrap(),
            Path::new("/data/launcher/homes/a_b c"),
        );

        // A remote host's version lives under *its* data directory, and the
        // launcher's own is not involved.
        let remote = remote_host("host-server", Some("/home/u/.local/share/dsh-launcher"));
        assert_eq!(
            remote
                .dedicated_home_path(Path::new("/data/launcher"), "0.1.0")
                .unwrap(),
            Path::new("/home/u/.local/share/dsh-launcher/homes/0.1.0"),
        );
        assert_eq!(
            remote.data_dir(Path::new("/data/launcher")).unwrap(),
            PathBuf::from("/home/u/.local/share/dsh-launcher"),
        );

        // The local host uses the launcher's own directory, whatever the record
        // says.
        assert_eq!(
            local.data_dir(Path::new("/data/launcher")).unwrap(),
            PathBuf::from("/data/launcher"),
        );

        // A remote host whose data directory has not been read off that machine
        // yet has none — paths must not be guessed from the launcher's own.
        let unresolved = remote_host("host-server", None);
        assert!(unresolved
            .dedicated_home_path(Path::new("/data/launcher"), "0.1.0")
            .is_err());
    }

    /// The bare rule, for a caller that already knows which data directory it
    /// means.
    #[test]
    fn dedicated_home_path_uses_homes_dir_and_sanitized_name() {
        let data_dir = Path::new("/data/launcher");
        assert_eq!(
            dedicated_home_path(data_dir, "0.1.0-rc.6"),
            Path::new("/data/launcher/homes/0.1.0-rc.6"),
        );
        // The name is sanitized with the SAME rule the installer used, so the
        // path deleted is provably the path created.
        assert_eq!(
            dedicated_home_path(data_dir, "a/b c"),
            Path::new("/data/launcher/homes/a_b c"),
        );
    }

    /// Case folding is the local volume's rule. A remote host is Linux: two
    /// paths that differ in case are two different paths.
    #[test]
    fn a_remote_host_does_not_fold_case_in_paths() {
        let remote = remote_host("host-server", Some("/data"));
        assert!(!remote.paths_equal(Path::new("/home/u/.dsh"), Path::new("/home/u/.DSH")));
        assert!(remote.paths_equal(Path::new("/home/u/.dsh"), Path::new("/home/u/.dsh")));

        #[cfg(target_os = "macos")]
        {
            let local = Host::local();
            assert!(
                local.paths_equal(Path::new("/Users/Me/.dsh"), Path::new("/users/me/.dsh")),
                "本机保持原有的折叠行为"
            );
        }
    }

    /// The same spelling on two hosts is two HOME records: `/home/u/.dsh`
    /// exists on both machines and only one of them is the local one.
    #[test]
    fn homes_on_two_hosts_are_not_merged_by_dedupe() {
        let mut cfg = Config::default();
        let local = Host::local();
        cfg.hosts.push(remote_host("host-server", Some("/data")));
        for (id, host_id) in [("h-local", LOCAL_HOST_ID), ("h-remote", "host-server")] {
            cfg.homes.push(DshHome {
                id: id.to_string(),
                name: id.to_string(),
                path: PathBuf::from("/srv/dsh"),
                host_id: host_id.to_string(),
            });
        }
        cfg.instances.push(DshInstance {
            id: "i-remote".to_string(),
            name: "i-remote".to_string(),
            version_id: "v1".to_string(),
            home_id: "h-remote".to_string(),
            host_id: "host-server".to_string(),
            env_overrides: BTreeMap::new(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        });

        dedupe_homes(&local, &mut cfg);

        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-local", "h-remote"],
            "两台机器上的同一个路径是两条记录"
        );
        assert_eq!(cfg.instances[0].home_id, "h-remote", "实例的归属不被改写");
    }

    /// Deduping the local host still works the way it always did.
    #[test]
    fn duplicate_homes_on_the_local_host_still_merge() {
        let mut cfg = Config::default();
        for id in ["h-one", "h-two"] {
            cfg.homes.push(DshHome {
                id: id.to_string(),
                name: id.to_string(),
                path: PathBuf::from("/srv/dsh/"),
                host_id: LOCAL_HOST_ID.to_string(),
            });
        }
        cfg.instances.push(DshInstance {
            id: "i1".to_string(),
            name: "i1".to_string(),
            version_id: "v1".to_string(),
            home_id: "h-two".to_string(),
            host_id: LOCAL_HOST_ID.to_string(),
            env_overrides: BTreeMap::new(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        });

        dedupe_homes(&Host::local(), &mut cfg);

        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-one"]
        );
        assert_eq!(cfg.instances[0].home_id, "h-one", "引用被重定向到留下的那条");
    }

    /// "This directory does not exist" is a statement about one machine's
    /// filesystem, so the sweep only judges the host it was asked about.
    #[test]
    fn the_orphan_sweep_leaves_another_hosts_records_alone() {
        let mut cfg = Config::default();
        let missing = PathBuf::from(format!("/dsh-missing-{}", uuid::Uuid::new_v4()));
        cfg.hosts.push(remote_host("host-server", Some("/data")));
        cfg.homes.push(DshHome {
            id: "h-local-orphan".to_string(),
            name: "本机的残留".to_string(),
            path: missing.clone(),
            host_id: LOCAL_HOST_ID.to_string(),
        });
        cfg.homes.push(DshHome {
            id: "h-remote".to_string(),
            name: "远端的 HOME".to_string(),
            path: PathBuf::from("/home/u/.dsh"),
            host_id: "host-server".to_string(),
        });

        cleanup_orphan_homes(&Host::local(), &mut cfg);

        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-remote"],
            "远端那条记录不能拿本机的文件系统去判"
        );
    }

    #[test]
    fn normalize_expands_tilde() {
        assert_eq!(normalize_dir_path("~/.dsh").unwrap(), home().join(".dsh"));
        assert_eq!(normalize_dir_path("~").unwrap(), home());
        assert_eq!(normalize_dir_path("  ~/.dsh  ").unwrap(), home().join(".dsh"));
    }

    #[test]
    fn normalize_drops_trailing_separator_and_dot_segments() {
        // `/a/b/` and `/a/b` must be one HOME, and `./` segments must not
        // produce a path with `~` or `.` baked in.
        assert_eq!(
            normalize_dir_path("/tmp/dsh-home/").unwrap(),
            PathBuf::from("/tmp/dsh-home")
        );
        assert_eq!(
            normalize_dir_path("/tmp/./dsh-home").unwrap(),
            PathBuf::from("/tmp/dsh-home")
        );
        assert_eq!(
            normalize_dir_path("/tmp/other/../dsh-home").unwrap(),
            PathBuf::from("/tmp/dsh-home")
        );
    }

    #[test]
    fn normalize_rejects_relative_and_tilde_user_paths() {
        // A relative path would mean "relative to the app's cwd" — the exact
        // bug that produced a literal `~` directory.
        assert!(normalize_dir_path("relative/dsh").is_err());
        assert!(normalize_dir_path("~/../x").is_ok(), ".. inside an absolute path is fine");
        assert!(normalize_dir_path("~someone/.dsh").is_err());
        assert!(normalize_dir_path("").is_err());
        assert!(normalize_dir_path("   ").is_err());
    }

    #[test]
    fn normalize_tilde_and_explicit_home_are_the_same_path() {
        // So that the ~/.dsh record and a typed /Users/me/.dsh dedupe to one
        // HOME instead of showing up twice.
        let explicit = home().join(".dsh");
        let via_tilde = normalize_dir_path("~/.dsh").unwrap();
        assert!(paths_equal(&explicit, &via_tilde));
    }

    #[test]
    fn bound_tool_is_stale_when_its_path_disappears() {
        // The binding is a resolution, not a preference: a path that no longer
        // exists must read as stale so the resolver re-probes instead of
        // failing to spawn. This is the nvm-switched-versions case.
        let dir = std::env::temp_dir().join(format!("bound-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("node");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();

        let binding = ToolchainBinding {
            node: Some(BoundTool {
                path: exe.clone(),
                version: "v22.0.0".to_string(),
            }),
            pnpm: None,
        };
        assert!(binding.node_is_fresh(None));
        assert!(!binding.pnpm_is_fresh(None), "an absent binding is never fresh");

        std::fs::remove_file(&exe).unwrap();
        assert!(!binding.node_is_fresh(None), "a vanished path must be stale");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hand_picked_tool_is_fresh_only_while_the_binding_is_that_file() {
        let dir = std::env::temp_dir().join(format!("manual-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let manual = dir.join("node");
        let detected = dir.join("other-node");
        std::fs::write(&manual, "#!/bin/sh\n").unwrap();
        std::fs::write(&detected, "#!/bin/sh\n").unwrap();

        let bound = |path: &Path| ToolchainBinding {
            node: Some(BoundTool {
                path: path.to_path_buf(),
                version: "v22.0.0".to_string(),
            }),
            pnpm: None,
        };

        // The user's choice is what the config must end up with, so a binding
        // that points anywhere else is stale and forces a re-resolve.
        assert!(bound(&manual).node_is_fresh(Some(&manual)));
        assert!(!bound(&detected).node_is_fresh(Some(&manual)));

        // A choice that stopped working falls back to detection — but the
        // binding it left behind is stale either way.
        std::fs::remove_file(&manual).unwrap();
        assert!(!bound(&manual).node_is_fresh(Some(&manual)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn toolchain_binding_defaults_are_absent() {
        // A config written before the binding existed must deserialize with
        // both tools unbound rather than failing to load.
        let binding = ToolchainBinding::default();
        assert!(binding.node.is_none());
        assert!(binding.pnpm.is_none());
        assert!(!binding.node_is_fresh(None));

        let raw = r#"{}"#;
        let parsed: ToolchainBinding = serde_json::from_str(raw).unwrap();
        assert!(parsed.node.is_none() && parsed.pnpm.is_none());
    }

    #[test]
    fn a_host_record_without_toolchain_fields_still_loads() {
        // Forward compatibility of the config file itself: both fields are
        // `#[serde(default)]`, so a host record written before per-host
        // toolchains existed loads with nothing probed and nothing chosen —
        // rather than failing and taking the whole config with it.
        let raw = r#"{"id":"host-lab","display_name":"服务器","ssh_target":"lab"}"#;
        let host: Host = serde_json::from_str(raw).unwrap();
        assert!(host.toolchain.node.is_none());
        assert!(!host.toolchain.node_is_fresh(None));
        assert!(host.toolchain_overrides.node.is_none());
        assert!(host.toolchain_overrides.pnpm.is_none());
    }

    #[test]
    fn a_legacy_toolchain_pair_moves_onto_the_local_host() {
        // The pair used to live under `settings`, and it was the local host's.
        // Loading such a config has to keep it: a hand-picked executable is the
        // user's choice, and nothing can re-derive it.
        let mut cfg = Config::default();
        migrate_hosts(&mut cfg);
        let document: serde_json::Value = serde_json::from_str(
            r#"{"settings":{
                 "toolchain":{"node":{"path":"/usr/bin/node","version":"v22.0.0"}},
                 "toolchain_overrides":{"pnpm":"/opt/pnpm/bin/pnpm"}}}"#,
        )
        .unwrap();

        assert!(adopt_legacy_toolchain(&document, &mut cfg));
        let local = cfg.local_host().unwrap();
        assert_eq!(local.toolchain.node.as_ref().unwrap().version, "v22.0.0");
        assert_eq!(
            local.toolchain_overrides.pnpm,
            Some(PathBuf::from("/opt/pnpm/bin/pnpm"))
        );

        // A record that already carries its own pair is newer than the legacy
        // one, and is left exactly as it is.
        assert!(!adopt_legacy_toolchain(&document, &mut cfg));
    }
}
