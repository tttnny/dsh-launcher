use crate::config::{
    new_id, sanitize_name, DshHome, DshInstance, DshVersion, LauncherSettings, RemoteVersion,
    SettingsPatch,
};
use crate::{process, AppState};
use std::collections::BTreeMap;
use tauri::{AppHandle, State};

// ---------------------------------------------------------------------------
// DSH_HOME
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_homes(state: State<'_, AppState>) -> Result<Vec<DshHome>, String> {
    Ok(state.config.lock().unwrap().homes.clone())
}

#[tauri::command]
pub fn create_home(
    state: State<'_, AppState>,
    name: String,
    path: String,
) -> Result<DshHome, String> {
    create_home_record(&state, &name, &path)
}

/// Shared helper: validates + creates a DSH_HOME record. The path is
/// normalized first (see [`crate::config::normalize_dir_path`]), so `~/.dsh`
/// and `/Users/me/.dsh` are recognized as one HOME instead of two records,
/// and a relative path is refused rather than resolved against the app's cwd.
/// If a HOME with the same path already exists, the existing record is
/// returned instead of creating a duplicate (prevents duplicate same-name
/// HOMEs when a dedicated HOME is requested repeatedly).
pub(crate) fn create_home_record(
    state: &State<'_, AppState>,
    name: &str,
    path: &str,
) -> Result<DshHome, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("名称不能为空".to_string());
    }
    let path_buf = crate::config::normalize_dir_path(path)?;

    // Reuse an existing HOME with the same path.
    {
        let cfg = state.config.lock().unwrap();
        if let Some(existing) = cfg
            .homes
            .iter()
            .find(|h| crate::config::paths_equal(&h.path, &path_buf))
        {
            return Ok(existing.clone());
        }
    }

    std::fs::create_dir_all(&path_buf).map_err(|e| format!("创建目录失败: {e}"))?;
    let home = DshHome {
        id: new_id("h"),
        name: name.to_string(),
        path: path_buf,
    };
    let mut cfg = state.config.lock().unwrap();
    cfg.homes.push(home.clone());
    save_state(state, &cfg)?;
    Ok(home)
}

#[tauri::command]
pub fn default_dedicated_home_path(
    state: State<'_, AppState>,
    name: String,
) -> Result<String, String> {
    Ok(state
        .data_dir
        .join("homes")
        .join(sanitize_name(&name))
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub fn remove_home(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    if cfg.instances.iter().any(|i| i.home_id == id) {
        return Err("该 DSH_HOME 仍被实例引用，无法删除".to_string());
    }
    cfg.homes.retain(|h| h.id != id);
    save_state(&state, &cfg)
}

// ---------------------------------------------------------------------------
// DSH versions
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_versions(state: State<'_, AppState>) -> Result<Vec<DshVersion>, String> {
    Ok(state.config.lock().unwrap().versions.clone())
}

/// Queries the npm registry for available @deepseek-ai/dsh versions with
/// their publish dates, then merges in GitHub-only `dsh-v*` release tags
/// (alpha builds are tagged on GitHub but not published to npm) marked with
/// `source: "github"`.
#[tauri::command]
pub async fn fetch_available_versions() -> Result<Vec<RemoteVersion>, String> {
    let versions_json = run_npm_view("@deepseek-ai/dsh", "versions").await?;
    let versions: Vec<String> =
        serde_json::from_str(&versions_json).map_err(|e| format!("解析版本列表失败: {e}"))?;

    let time_json = run_npm_view("@deepseek-ai/dsh", "time").await?;
    // npm >= 9 returns `time` as `[{ "created": ..., "<version>": "<date>" }]`
    // (array wrapping one object); older npm returned the object directly.
    let time_map: BTreeMap<String, serde_json::Value> =
        match serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&time_json) {
            Ok(map) => map,
            Err(_) => {
                let arr: Vec<BTreeMap<String, serde_json::Value>> =
                    serde_json::from_str(&time_json)
                        .map_err(|e| format!("解析发布时间失败: {e}"))?;
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

async fn run_npm_view(pkg: &str, field: &str) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(process::npm());
    // Keep the listing consistent with the installer's npm probe: both honor
    // the DSH_NPM_REGISTRY mirror, so a mirror user doesn't see official-
    // registry data in the list but mirror data at install time (or vice
    // versa when the mirror lags).
    crate::proxy::apply_to_command(&mut cmd);
    cmd.args(["view", pkg, field, "--json"]);
    if let Some(registry) = crate::toolchain::registry_mirror() {
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
) -> Result<(), String> {
    let (version, instance_ids) = {
        let cfg = state.config.lock().unwrap();
        let Some(version) = cfg.versions.iter().find(|v| v.id == id).cloned() else {
            return Err("版本不存在".to_string());
        };
        let inst_ids: Vec<String> = cfg
            .instances
            .iter()
            .filter(|i| i.version_id == id)
            .map(|i| i.id.clone())
            .collect();
        (version, inst_ids)
    };

    // Stop and cascade delete associated instances
    for inst_id in &instance_ids {
        if state.running.lock().await.contains_key(inst_id) {
            let _ = process::stop_instance_process(&app, &state, inst_id).await;
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
        save_state(&state, &cfg)?;
    }

    // Best-effort removal of the install directory.
    let _ = std::fs::remove_dir_all(&version.dir);
    Ok(())
}

// ---------------------------------------------------------------------------
// Instances
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_instances(state: State<'_, AppState>) -> Result<Vec<DshInstance>, String> {
    Ok(state.config.lock().unwrap().instances.clone())
}

#[tauri::command]
pub fn update_instance(
    state: State<'_, AppState>,
    input: DshInstance,
) -> Result<DshInstance, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err("实例名称不能为空".to_string());
    }
    let mut cfg = state.config.lock().unwrap();
    if cfg
        .instances
        .iter()
        .any(|i| i.name == name && i.id != input.id)
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
    // The icon is managed by set/clear_instance_icon and the port by
    // set_instance_port, not by this form payload; preserve whatever is
    // currently stored (the frontend spreads a possibly stale instance).
    updated.icon = cfg.instances[pos].icon.clone();
    updated.port = cfg.instances[pos].port;
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

#[tauri::command(rename_all = "snake_case")]
pub fn list_profiles(state: State<'_, AppState>, home_id: String) -> Result<Vec<String>, String> {
    let cfg = state.config.lock().unwrap();
    let home = cfg
        .homes
        .iter()
        .find(|h| h.id == home_id)
        .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
    let profiles_dir = crate::profile::profiles_dir(&home.path);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&profiles_dir) {
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
    Ok(out)
}

/// Creates a new profile by copying the `__temp__` template from dedicated homes.
/// Never creates or overrides `__temp__` in user default ~/.dsh/profiles (issue #5).
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
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        let is_user_default = home.id == "home-user-dsh"
            || std::env::var("HOME")
                .map(|h| home.path == std::path::PathBuf::from(h).join(".dsh"))
                .unwrap_or(false);
        (home.path.clone(), is_user_default)
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
pub fn copy_profile(
    state: State<'_, AppState>,
    home_id: String,
    source: String,
    name: String,
) -> Result<String, String> {
    let name = name.trim().to_string();
    crate::profile::validate_profile_name(&name)?;

    let profiles_dir = {
        let cfg = state.config.lock().unwrap();
        let home = cfg
            .homes
            .iter()
            .find(|h| h.id == home_id)
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        crate::profile::profiles_dir(&home.path)
    };

    let from = profiles_dir.join(&source);
    if !from.is_dir() {
        return Err(format!("Profile「{source}」不存在"));
    }
    let to = profiles_dir.join(&name);
    if to.exists() {
        return Err(format!("Profile「{name}」已存在"));
    }

    copy_dir_recursive(&from, &to).map_err(|e| format!("复制 Profile 失败: {e}"))?;
    Ok(name)
}

/// Renames a profile directory inside the given HOME.
#[tauri::command(rename_all = "snake_case")]
pub fn rename_profile(
    state: State<'_, AppState>,
    home_id: String,
    old_name: String,
    new_name: String,
) -> Result<String, String> {
    crate::profile::validate_profile_name(&new_name)?;
    let new_name = new_name.trim().to_string();

    let profiles_dir = {
        let cfg = state.config.lock().unwrap();
        let home = cfg
            .homes
            .iter()
            .find(|h| h.id == home_id)
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        crate::profile::profiles_dir(&home.path)
    };

    let from = profiles_dir.join(&old_name);
    if !from.is_dir() {
        return Err(format!("Profile「{old_name}」不存在"));
    }
    let to = profiles_dir.join(&new_name);
    if to.exists() {
        return Err(format!("Profile「{new_name}」已存在"));
    }

    std::fs::rename(&from, &to).map_err(|e| format!("重命名 Profile 失败: {e}"))?;

    // Keep the instance's default/last profile references in sync.
    {
        let mut cfg = state.config.lock().unwrap();
        for inst in cfg.instances.iter_mut() {
            if inst.home_id == home_id {
                if inst.default_profile.as_deref() == Some(old_name.as_str()) {
                    inst.default_profile = Some(new_name.clone());
                }
                if inst.last_profile.as_deref() == Some(old_name.as_str()) {
                    inst.last_profile = Some(new_name.clone());
                }
            }
        }
        save_state(&state, &cfg)?;
    }

    Ok(new_name)
}

/// Deletes a profile directory inside the given HOME. The default/last profile
/// references on instances using this HOME are cleared when they point at the
/// removed profile.
#[tauri::command(rename_all = "snake_case")]
pub fn delete_profile(
    state: State<'_, AppState>,
    home_id: String,
    name: String,
) -> Result<(), String> {
    let profiles_dir = {
        let cfg = state.config.lock().unwrap();
        let home = cfg
            .homes
            .iter()
            .find(|h| h.id == home_id)
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
        crate::profile::profiles_dir(&home.path)
    };

    let target = profiles_dir.join(&name);
    if !target.is_dir() {
        return Err(format!("Profile「{name}」不存在"));
    }
    if name == "__temp__" || name == "node_modules" {
        return Err(format!("「{name}」为保留名称，不能删除"));
    }

    std::fs::remove_dir_all(&target).map_err(|e| format!("删除 Profile 失败: {e}"))?;

    {
        let mut cfg = state.config.lock().unwrap();
        for inst in cfg.instances.iter_mut() {
            if inst.home_id == home_id {
                if inst.default_profile.as_deref() == Some(name.as_str()) {
                    inst.default_profile = None;
                }
                if inst.last_profile.as_deref() == Some(name.as_str()) {
                    inst.last_profile = None;
                }
            }
        }
        save_state(&state, &cfg)?;
    }

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
    let profile_dir = crate::profile::profile_dir(&home_path, &profile)?;
    let report =
        crate::doctor::inspect(&instance_id, &profile, &version_dir, &version, &profile_dir);
    crate::doctor::log_report(&report);
    Ok(report)
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
    // Findings never block the launch.
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

#[tauri::command]
pub async fn open_instance_window(
    _app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let entry = state.running.lock().await.get(&id).map(|r| r.url.clone());
    let Some(url) = entry.flatten() else {
        return Err("实例未在运行或尚未就绪".to_string());
    };
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
#[tauri::command]
pub fn open_instance_terminal(
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<String, String> {
    let (cfg_home, cfg_inst, cfg_ver) = {
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
        (home, inst, ver)
    };
    let terminal = state.config.lock().unwrap().settings.terminal.clone();
    // The terminal's wrapper must boot the same CLI the instance does, so it
    // takes the instance's own flag.
    let preserve_symlinks = cfg_inst.preserve_symlinks;
    std::fs::create_dir_all(&cfg_home.path).map_err(|e| format!("创建 HOME 目录失败: {e}"))?;
    let home_str = cfg_home.path.to_string_lossy().to_string();

    // Prepend the instance's DSH version bin directory to PATH so `dsh` is
    // immediately available; the launch module owns the wrapper + PATH policy.
    let version_dir: Option<std::path::PathBuf> = cfg_ver
        .as_ref()
        .map(|v| std::path::PathBuf::from(&v.dir));
    let path_dirs = crate::launch::terminal_path_dirs(version_dir.as_deref(), preserve_symlinks);

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
    let scripts_dir = state.data_dir.join("scripts");
    let _ = std::fs::create_dir_all(&scripts_dir);
    let term_script = scripts_dir.join(format!("term_{}.sh", instance_id));
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

    if terminal == "ghostty" {
        // Ghostty on macOS: -na opens a new instance and passes --command to execute the script.
        let status = std::process::Command::new("open")
            .arg("-na")
            .arg("Ghostty")
            .arg("--args")
            .arg(format!("--title={label}"))
            .arg(format!("--command={}", shell_quote(&term_script.to_string_lossy())))
            .spawn()
            .and_then(|mut c| c.wait());

        // Bring Ghostty to front
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg("tell application \"Ghostty\" to activate")
            .output();

        match status {
            Ok(_) => Ok(label),
            Err(e) => Err(format!("打开 Ghostty 失败: {e}")),
        }
    } else {
        // Terminal.app. A cold-started Terminal always opens a default
        // startup window, so `activate` + `do script` would produce two
        // windows. Instead, on cold start we wait for that startup window
        // and run the script inside it.
        let escaped_path = term_script.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
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
            do script \"exec \\\"{escaped_path}\\\"\" in tab 1 of front window\n\
            return\n\
            end if\n\
            end if\n\
            do script \"exec \\\"{escaped_path}\\\"\"\n\
            activate\n\
            end tell"
        );
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("打开终端失败: {e}"))?;
        if out.status.success() {
            Ok(label)
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

/// Single-quote a value for sh -c / do-script embedding.
fn shell_quote(v: &str) -> String {
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

/// Reveals one instance's runtime log (`<data_dir>/logs/<instance_id>.log`)
/// in the file manager with the file selected.
#[tauri::command]
pub fn open_instance_log(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    instance_id: String,
) -> Result<String, String> {
    {
        let cfg = state.config.lock().unwrap();
        if !cfg.instances.iter().any(|i| i.id == instance_id) {
            return Err("实例不存在".to_string());
        }
    }
    let log_dir = state.data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
    reveal_log_file(&app, log_dir.join(format!("{instance_id}.log")))
}

/// Opens the DSH_HOME directory of one instance in the file manager.
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
        cfg.homes
            .iter()
            .find(|h| h.id == inst.home_id)
            .map(|h| h.path.clone())
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?
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
#[tauri::command]
pub fn open_home_directory(
    state: State<'_, AppState>,
    home_id: String,
) -> Result<String, String> {
    let home = {
        let cfg = state.config.lock().unwrap();
        cfg.homes
            .iter()
            .find(|h| h.id == home_id)
            .map(|h| h.path.clone())
            .ok_or_else(|| "DSH_HOME 不存在".to_string())?
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

pub(crate) fn save_state(
    state: &State<'_, AppState>,
    cfg: &crate::config::Config,
) -> Result<(), String> {
    crate::config::save_config(&state.config_path, cfg)
}
