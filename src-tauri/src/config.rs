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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshVersion {
    pub id: String,
    pub version: String,
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshInstance {
    pub id: String,
    pub name: String,
    pub version_id: String,
    pub home_id: String,
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

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub homes: Vec<DshHome>,
    #[serde(default)]
    pub versions: Vec<DshVersion>,
    #[serde(default)]
    pub instances: Vec<DshInstance>,
    #[serde(default)]
    pub settings: LauncherSettings,
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
    match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<Config>(&raw) {
            Ok(mut cfg) => {
                let data_dir = path.parent().unwrap_or(Path::new("."));
                dedupe_homes(&mut cfg);
                cleanup_orphan_homes(&mut cfg);
                ensure_user_dsh_home(&mut cfg);
                let changed = migrate_instances_to_dedicated_homes(data_dir, &mut cfg);
                if changed {
                    let _ = save_config(path, &cfg);
                }
                cfg
            }
            Err(err) => {
                // Back up the broken file and start fresh.
                let _ = fs::copy(path, path.with_extension("json.bak"));
                eprintln!("dsh-launcher: config corrupted, backed up: {err}");
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

/// Removes HOME records whose directory no longer exists AND that are not
/// referenced by any instance (a stale placeholder from an interrupted task
/// or a manually deleted folder). Directories that still exist are kept even
/// if unreferenced (they may be user-managed).
pub fn cleanup_orphan_homes(cfg: &mut Config) {
    let orphans: Vec<String> = cfg
        .homes
        .iter()
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
pub fn ensure_user_dsh_home(cfg: &mut Config) {
    let home_dir = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .map(std::path::PathBuf::from);
    let Some(home_dir) = home_dir else { return };
    let dsh = home_dir.join(".dsh");
    if !dsh.exists() {
        return;
    }
    if cfg.homes.iter().any(|h| paths_equal(&h.path, &dsh)) {
        return;
    }
    cfg.homes.push(DshHome {
        id: "home-user-dsh".to_string(),
        name: "用户默认 (~/.dsh)".to_string(),
        path: dsh,
    });
}

/// Path equality. APFS volumes are case-insensitive by default, so the
/// comparison folds case on macOS (matching how the filesystem itself
/// resolves paths); elsewhere plain equality is used.
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

/// Merge HOME records that point at the same path, keeping the first and
/// redirecting instance references to the surviving home id (cleans up
/// duplicates created before path-based reuse existed).
pub fn dedupe_homes(cfg: &mut Config) {
    if cfg.homes.len() < 2 {
        return;
    }
    let mut kept: Vec<DshHome> = Vec::new();
    let mut redirect: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for home in &cfg.homes {
        if let Some(existing) = kept.iter().find(|e| paths_equal(&e.path, &home.path)) {
            redirect.insert(home.id.clone(), existing.id.clone());
        } else {
            kept.push(home.clone());
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
    cfg.homes = kept;
}

/// Enforces the 1-version-1-instance contract and migrates instances to dedicated
/// homes under `<data_dir>/homes/<version>` (issue #3, #4).
pub fn migrate_instances_to_dedicated_homes(data_dir: &Path, cfg: &mut Config) -> bool {
    let mut modified = false;

    // 1. Remove instances whose version no longer exists.
    let old_inst_len = cfg.instances.len();
    cfg.instances.retain(|i| cfg.versions.iter().any(|v| v.id == i.version_id));
    if cfg.instances.len() != old_inst_len {
        modified = true;
    }

    // 2. Ensure each version has its dedicated home and 1:1 instance.
    for v in &cfg.versions {
        let expected_home_path = data_dir.join("homes").join(sanitize_name(&v.version));

        // Find existing home by path or create it.
        let home_id = if let Some(h) = cfg.homes.iter().find(|h| paths_equal(&h.path, &expected_home_path)) {
            h.id.clone()
        } else {
            let hid = new_id("h");
            cfg.homes.push(DshHome {
                id: hid.clone(),
                name: v.version.clone(),
                path: expected_home_path.clone(),
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
