use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshHome {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshVersion {
    pub id: String,
    pub version: String,
    pub dir: PathBuf,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DshInstance {
    pub id: String,
    pub name: String,
    pub version_id: String,
    pub home_id: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub env_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub last_profile: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

pub const LOCAL_HOST_ID: &str = "host-local";

pub const USER_DSH_HOME_ID: &str = "home-user-dsh";
pub const USER_DSH_HOME_NAME: &str = "用户默认 (~/.dsh)";

pub fn user_dsh_home_id(host: &Host) -> String {
    if host.builtin {
        USER_DSH_HOME_ID.to_string()
    } else {
        format!("{USER_DSH_HOME_ID}-{}", host.id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub ssh_target: String,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub toolchain: ToolchainBinding,
    #[serde(default)]
    pub toolchain_overrides: ToolchainOverrides,
}

impl Host {
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

    pub fn data_dir(&self, launcher_data_dir: &Path) -> Result<PathBuf, String> {
        if self.builtin {
            return Ok(launcher_data_dir.to_path_buf());
        }
        let dir = self
            .data_dir
            .clone()
            .ok_or_else(|| format!("主机「{}」还没有远端数据目录", self.display_name))?;
        if !dir.is_absolute() {
            return Err(format!(
                "主机「{}」的远端数据目录还没在那台机器上确定（现在记的是 {}）：在设置里对它点一次「测试连接」，或填一个绝对路径",
                self.display_name,
                dir.display()
            ));
        }
        Ok(dir)
    }

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

    pub fn paths_equal(&self, a: &Path, b: &Path) -> bool {
        if self.builtin {
            paths_equal(a, b)
        } else {
            a == b
        }
    }

    pub fn local_paths_only(&self, what: &str) -> Result<(), String> {
        if self.builtin {
            Ok(())
        } else {
            Err(format!(
                "「{}」上的{what}不在本机，启动器不会在本机动它",
                self.display_name
            ))
        }
    }
}

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
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub last_instance_id: Option<String>,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub proxy_enabled: bool,
    #[serde(default = "default_proxy_url")]
    pub proxy_url: String,
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
    #[serde(default = "default_no_proxy")]
    pub no_proxy: String,
    #[serde(default)]
    pub proxy_apply_dsh: bool,
    #[serde(default = "default_terminal")]
    pub terminal: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolchainOverrides {
    #[serde(default)]
    pub node: Option<PathBuf>,
    #[serde(default)]
    pub pnpm: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolchainBinding {
    #[serde(default)]
    pub node: Option<BoundTool>,
    #[serde(default)]
    pub pnpm: Option<BoundTool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundTool {
    pub path: PathBuf,
    pub version: String,
}

impl ToolchainBinding {
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
    #[serde(default)]
    pub hosts: Vec<Host>,
}

impl Default for Config {
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
    pub fn local_host(&self) -> Option<&Host> {
        self.hosts.iter().find(|h| h.builtin)
    }
}

pub fn resolve_host<'a>(cfg: &'a Config, host_id: Option<&str>) -> Result<&'a Host, String> {
    match host_id.map(str::trim).filter(|id| !id.is_empty()) {
        None => cfg
            .local_host()
            .ok_or_else(|| "配置里没有本机记录".to_string()),
        Some(id) => cfg
            .hosts
            .iter()
            .find(|h| h.id == id)
            .ok_or_else(|| format!("主机不存在: {id}")),
    }
}

pub fn migrate_hosts(cfg: &mut Config) -> bool {
    let mut changed = false;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteVersion {
    pub version: String,
    pub released_at: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SettingsPatch {
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
    Unreachable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub id: String,
    pub state: InstanceState,
    pub url: Option<String>,
    #[serde(default)]
    pub remote_url: Option<String>,
    pub profile: Option<String>,
    pub exit_code: Option<i32>,
}

pub fn load_config(path: &Path) -> Result<Config, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => Some(raw),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("读取配置失败: {err}")),
    };
    let document = match &raw {
        Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(document) => document,
            Err(_) => return Ok(backup_and_reset(path, "配置不是合法的 JSON")),
        },
        None => serde_json::Value::Object(serde_json::Map::new()),
    };
    match serde_json::from_value::<Config>(document.clone()) {
        Ok(mut cfg) => {
            let data_dir = path.parent().unwrap_or(Path::new("."));
            let hosts_changed = migrate_hosts(&mut cfg);
            let adopted = adopt_legacy_toolchain(&document, &mut cfg);
            let local = cfg.local_host().cloned().unwrap_or_else(Host::local);
            dedupe_homes(&local, &mut cfg);
            cleanup_orphan_homes(&local, &mut cfg);
            ensure_user_dsh_home(&local, &mut cfg);
            let changed = hosts_changed
                || adopted
                || migrate_instances_to_dedicated_homes(&local, data_dir, &mut cfg);
            if changed || raw.is_none() {
                let _ = save_config(path, &cfg);
            }
            Ok(cfg)
        }
        Err(err) => Ok(backup_and_reset(path, &err.to_string())),
    }
}

fn backup_and_reset(path: &Path, reason: &str) -> Config {
    let _ = fs::copy(path, path.with_extension("json.bak"));
    eprintln!("dsh-launcher: config corrupted, backed up: {reason}");
    Config::default()
}

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

pub fn cleanup_orphan_homes(host: &Host, cfg: &mut Config) {
    if !host.builtin {
        return;
    }
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

pub fn ensure_user_dsh_home(host: &Host, cfg: &mut Config) {
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
        id: USER_DSH_HOME_ID.to_string(),
        name: USER_DSH_HOME_NAME.to_string(),
        path: dsh,
        host_id: host.id.clone(),
    });
}

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

pub(crate) fn dedicated_home_path(data_dir: &Path, version: &str) -> PathBuf {
    data_dir.join("homes").join(sanitize_name(version))
}

pub fn dedupe_homes(host: &Host, cfg: &mut Config) {
    let mut redirect: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    {
        let mut kept: Vec<&DshHome> = Vec::new();
        for home in cfg.homes.iter().filter(|h| h.host_id == host.id) {
            if let Some(existing) = kept.iter().find(|e| host.paths_equal(&e.path, &home.path)) {
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

pub fn migrate_instances_to_dedicated_homes(
    host: &Host,
    data_dir: &Path,
    cfg: &mut Config,
) -> bool {
    let mut modified = false;

    let old_inst_len = cfg.instances.len();
    cfg.instances
        .retain(|i| cfg.versions.iter().any(|v| v.id == i.version_id));
    if cfg.instances.len() != old_inst_len {
        modified = true;
    }

    for v in cfg.versions.iter().filter(|v| v.host_id == host.id) {
        let Some(inst) = cfg.instances.iter_mut().find(|i| i.version_id == v.id) else {
            continue;
        };
        if cfg.homes.iter().any(|h| h.id == inst.home_id) {
            continue;
        }
        let expected_home_path = dedicated_home_path(data_dir, &v.version);
        let home_id = match cfg
            .homes
            .iter()
            .find(|h| h.host_id == host.id && host.paths_equal(&h.path, &expected_home_path))
        {
            Some(h) => h.id.clone(),
            None => {
                let hid = new_id("h");
                cfg.homes.push(DshHome {
                    id: hid.clone(),
                    name: v.version.clone(),
                    path: expected_home_path,
                    host_id: host.id.clone(),
                });
                hid
            }
        };
        inst.home_id = home_id;
        modified = true;
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

pub fn normalize_dir_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空".to_string());
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let expanded: PathBuf = if trimmed == "~" {
        home.clone()
            .ok_or_else(|| "无法解析 ~：未设置 HOME".to_string())?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.clone()
            .ok_or_else(|| "无法解析 ~：未设置 HOME".to_string())?
            .join(rest)
    } else if trimmed.starts_with('~') {
        return Err(format!("不支持 ~用户名 形式的路径：{trimmed}"));
    } else {
        PathBuf::from(trimmed)
    };

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

    fn home() -> PathBuf {
        std::env::var_os("HOME").map(PathBuf::from).unwrap()
    }

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

    #[test]
    fn only_the_local_hosts_paths_may_be_touched_from_here() {
        assert!(Host::local().local_paths_only("DSH_HOME 目录").is_ok());

        let remote = remote_host("host-lab", Some("/home/demo/.local/share/dsh-launcher"));
        let err = remote.local_paths_only("版本目录").unwrap_err();
        assert!(err.contains("远端 host-lab"), "{err}");
        assert!(err.contains("版本目录"), "{err}");
        assert!(err.contains("不在本机"), "{err}");
    }

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

    #[test]
    fn old_config_records_join_the_local_host() {
        let dir = scratch("hosts-migrate");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, OLD_CONFIG_JSON).unwrap();

        let cfg = load_config(&config_path).unwrap();

        assert_eq!(cfg.hosts.len(), 1, "只有本机一条");
        let local = cfg.local_host().expect("本机必须在");
        assert_eq!(local.id, LOCAL_HOST_ID);
        assert!(local.builtin, "本机是内置的那一条，不能删除");
        assert_eq!(local.display_name, "本机");

        for h in &cfg.homes {
            assert_eq!(h.host_id, LOCAL_HOST_ID, "DSH_HOME {} 归本机", h.id);
        }
        for v in &cfg.versions {
            assert_eq!(v.host_id, LOCAL_HOST_ID, "版本 {} 归本机", v.id);
        }
        for i in &cfg.instances {
            assert_eq!(i.host_id, LOCAL_HOST_ID, "实例 {} 归本机", i.id);
        }

        let home = cfg.homes.iter().find(|h| h.id == "h-dedicated").unwrap();
        assert_eq!(home.name, "0.1.0-rc.6");
        assert_eq!(home.path, PathBuf::from("/data/launcher/homes/0.1.0-rc.6"));
        let version = cfg.versions.iter().find(|v| v.id == "v-rc6").unwrap();
        assert_eq!(version.version, "0.1.0-rc.6");
        assert_eq!(
            version.dir,
            PathBuf::from("/data/launcher/versions/0.1.0-rc.6")
        );
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
        assert_eq!(cfg.settings.theme, "dark");
        assert_eq!(cfg.settings.log_level, "debug");
        assert!(cfg.settings.proxy_enabled);
        assert!(cfg.settings.proxy_apply_dsh);
        assert!(!cfg.settings.minimize_to_tray);
        assert!(cfg.settings.autostart);
        assert_eq!(cfg.settings.terminal, "ghostty");

        assert_eq!(
            local.toolchain.node.as_ref().map(|b| b.path.clone()),
            Some(PathBuf::from("/opt/node/bin/node"))
        );
        assert_eq!(
            local.toolchain_overrides.pnpm,
            Some(PathBuf::from("/opt/pnpm/bin/pnpm"))
        );
        assert_eq!(cfg.settings.last_instance_id.as_deref(), Some("i-web"));

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

    #[test]
    fn a_fresh_config_already_has_the_local_host() {
        let cfg = Config::default();
        let local = cfg.local_host().expect("本机必须在");
        assert_eq!(local.id, LOCAL_HOST_ID);
        assert!(local.builtin);
        assert!(resolve_host(&cfg, None).is_ok(), "不指定主机时解析到本机");
    }

    #[test]
    fn a_config_without_a_hosts_key_still_resolves_the_local_host() {
        let dir = scratch("hosts-missing-key");
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, r#"{}"#).unwrap();

        let cfg = load_config(&config_path).unwrap();

        assert_eq!(cfg.hosts.len(), 1);
        assert!(resolve_host(&cfg, Some(LOCAL_HOST_ID)).is_ok());
        let err = resolve_host(&cfg, Some("host-nope")).unwrap_err();
        assert!(err.contains("主机不存在"), "err: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dedicated_home_paths_are_computed_on_their_host() {
        let local = Host::local();
        assert_eq!(
            local
                .dedicated_home_path(Path::new("/data/launcher"), "0.1.0-rc.6")
                .unwrap(),
            Path::new("/data/launcher/homes/0.1.0-rc.6"),
        );
        assert_eq!(
            local
                .dedicated_home_path(Path::new("/data/launcher"), "a/b c")
                .unwrap(),
            Path::new("/data/launcher/homes/a_b c"),
        );

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

        assert_eq!(
            local.data_dir(Path::new("/data/launcher")).unwrap(),
            PathBuf::from("/data/launcher"),
        );

        let unresolved = remote_host("host-server", None);
        assert!(unresolved
            .dedicated_home_path(Path::new("/data/launcher"), "0.1.0")
            .is_err());
    }

    #[test]
    fn dedicated_home_path_uses_homes_dir_and_sanitized_name() {
        let data_dir = Path::new("/data/launcher");
        assert_eq!(
            dedicated_home_path(data_dir, "0.1.0-rc.6"),
            Path::new("/data/launcher/homes/0.1.0-rc.6"),
        );
        assert_eq!(
            dedicated_home_path(data_dir, "a/b c"),
            Path::new("/data/launcher/homes/a_b c"),
        );
    }

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
        assert_eq!(
            cfg.instances[0].home_id, "h-one",
            "引用被重定向到留下的那条"
        );
    }

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

        let remote = cfg.hosts[0].clone();
        cleanup_orphan_homes(&remote, &mut cfg);
        assert_eq!(
            cfg.homes.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h-remote"]
        );
    }

    #[test]
    fn normalize_expands_tilde() {
        assert_eq!(normalize_dir_path("~/.dsh").unwrap(), home().join(".dsh"));
        assert_eq!(normalize_dir_path("~").unwrap(), home());
        assert_eq!(
            normalize_dir_path("  ~/.dsh  ").unwrap(),
            home().join(".dsh")
        );
    }

    #[test]
    fn normalize_drops_trailing_separator_and_dot_segments() {
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
        assert!(normalize_dir_path("relative/dsh").is_err());
        assert!(
            normalize_dir_path("~/../x").is_ok(),
            ".. inside an absolute path is fine"
        );
        assert!(normalize_dir_path("~someone/.dsh").is_err());
        assert!(normalize_dir_path("").is_err());
        assert!(normalize_dir_path("   ").is_err());
    }

    #[test]
    fn normalize_tilde_and_explicit_home_are_the_same_path() {
        let explicit = home().join(".dsh");
        let via_tilde = normalize_dir_path("~/.dsh").unwrap();
        assert!(paths_equal(&explicit, &via_tilde));
    }

    #[test]
    fn bound_tool_is_stale_when_its_path_disappears() {
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
        assert!(
            !binding.pnpm_is_fresh(None),
            "an absent binding is never fresh"
        );

        std::fs::remove_file(&exe).unwrap();
        assert!(
            !binding.node_is_fresh(None),
            "a vanished path must be stale"
        );

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

        assert!(bound(&manual).node_is_fresh(Some(&manual)));
        assert!(!bound(&detected).node_is_fresh(Some(&manual)));

        std::fs::remove_file(&manual).unwrap();
        assert!(!bound(&manual).node_is_fresh(Some(&manual)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn toolchain_binding_defaults_are_absent() {
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
        let raw = r#"{"id":"host-lab","display_name":"服务器","ssh_target":"lab"}"#;
        let host: Host = serde_json::from_str(raw).unwrap();
        assert!(host.toolchain.node.is_none());
        assert!(!host.toolchain.node_is_fresh(None));
        assert!(host.toolchain_overrides.node.is_none());
        assert!(host.toolchain_overrides.pnpm.is_none());
    }

    #[test]
    fn a_legacy_toolchain_pair_moves_onto_the_local_host() {
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

        assert!(!adopt_legacy_toolchain(&document, &mut cfg));
    }
}
