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
    /// Pass `--preserve-symlinks` to every DSH process the launcher spawns.
    ///
    /// Needed only when a profile depends on a `link:`-ed plugin: the flag
    /// keeps that plugin's module URL on its profile path, which is what DSH's
    /// kernel-package routing keys on. Off by default because it makes Node
    /// resolve one package through several distinct paths, so
    /// `@deepseek-ai/dsh-app-boot` gets loaded more than once and every DSH
    /// settings write fails with `profile reload requires the root Include
    /// entry` — which bricks the Web UI's first-run notice and the whole
    /// Settings surface.
    #[serde(default)]
    pub preserve_symlinks: bool,
    /// Where the launcher's own toolchain resolution last landed: the absolute
    /// paths of the `node` and `pnpm` it will drive DSH with. This is a probe
    /// *cache*, not a user preference — it is rewritten at startup and by the
    /// setup page's re-check, and a stored path that no longer exists is
    /// re-probed rather than reported as an error. Persisting it means a
    /// spawned process no longer depends on the PATH the app happened to
    /// inherit at launch.
    #[serde(default)]
    pub toolchain: ToolchainBinding,
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
    fn is_fresh(&self, tool: &Option<BoundTool>) -> bool {
        match tool {
            Some(bound) => bound.path.is_file(),
            None => false,
        }
    }

    pub fn node_is_fresh(&self) -> bool {
        self.is_fresh(&self.node)
    }

    pub fn pnpm_is_fresh(&self) -> bool {
        self.is_fresh(&self.pnpm)
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
            preserve_symlinks: false,
            toolchain: ToolchainBinding::default(),
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
    #[serde(default)]
    pub preserve_symlinks: Option<bool>,
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
        assert!(binding.node_is_fresh());
        assert!(!binding.pnpm_is_fresh(), "an absent binding is never fresh");

        std::fs::remove_file(&exe).unwrap();
        assert!(!binding.node_is_fresh(), "a vanished path must be stale");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn toolchain_binding_defaults_are_absent() {
        // A config written before the binding existed must deserialize with
        // both tools unbound rather than failing to load.
        let binding = ToolchainBinding::default();
        assert!(binding.node.is_none());
        assert!(binding.pnpm.is_none());
        assert!(!binding.node_is_fresh());

        let raw = r#"{}"#;
        let parsed: ToolchainBinding = serde_json::from_str(raw).unwrap();
        assert!(parsed.node.is_none() && parsed.pnpm.is_none());
    }

    #[test]
    fn settings_without_toolchain_field_still_load() {
        // Forward compatibility of the config file itself: `toolchain` is
        // `#[serde(default)]`, so an old config.json keeps loading.
        let raw = r#"{"locale":"zh-CN"}"#;
        let settings: LauncherSettings = serde_json::from_str(raw).unwrap();
        assert!(settings.toolchain.node.is_none());
    }
}
