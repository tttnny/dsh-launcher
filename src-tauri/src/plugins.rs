use crate::config::{DshHome, Host};
use crate::native::{
    BundleItem, NativeChangeResult, NativeClient, NativeTransport, PluginView, PluginViewMode,
    ReqwestTransport,
};
use crate::AppState;
use serde::Deserialize;
use std::sync::Arc;
use tauri::State;

const GITHUB_CLIENT_ID: &str = "Ov23li6vtlVd83282YL6";

pub(crate) fn github_api_url(path: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("https://api.github.com{path}{sep}client_id={GITHUB_CLIENT_ID}")
}

fn http_transport() -> Result<Arc<dyn NativeTransport>, String> {
    Ok(Arc::new(ReqwestTransport::new()?))
}

fn http_client() -> Result<reqwest::Client, String> {
    crate::proxy::apply(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("dsh-launcher")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

pub(crate) async fn fetch_json(url: &str, cap: usize) -> Result<serde_json::Value, String> {
    let client = http_client()?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("请求失败 {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("请求失败 {url}: HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取响应失败 {url}: {e}"))?;
    if bytes.len() > cap {
        return Err(format!("响应过大 {url}"));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("解析 JSON 失败 {url}: {e}"))
}

/// Lists the **bundles（补丁层）** of one profile of `(home_id, profile)`.
///
/// The bundles are the whole answer: a bundle is what the profile selects and
/// what DSH mounts as one patch layer, and it is the only unit that carries a
/// version, an availability and a removal rule. The composition rows inside a
/// layer are DSH's own detail, not a control surface the launcher hands out one
/// switch at a time — the page states how many rows a layer contributes and
/// manages the layer.
///
/// While an instance runs that profile the list is the instance's own
/// **native plugin manager** answer (`POST /api/pluginManager/listBundles`) —
/// the running DSH owns plugin state (ADR-0001), so the launcher neither reads
/// files to build this list nor writes one to change it. An **adopted** process
/// with no usable token and an unreachable one take the read-only path; their
/// `reason` differs so the two are distinguishable, and so is "not running".
///
/// A remote instance is asked at the address **this Mac** reaches it at: the
/// launcher's own port forward, re-confirmed (and rebuilt, if it died) before
/// every call ([`crate::process::reachable_url_for`], ADR-0008). The projection
/// for the read-only path is read on the machine that owns the profile, and a
/// machine the launcher cannot reach answers with its own reason instead.
///
/// The instance is resolved from the selectors: the same DSH_HOME, running the
/// selected profile (see [`crate::process::running_for_profile`]). `instance_id`
/// is the instance the page was opened from, and it wins when it is itself
/// running that profile — one DSH_HOME and one profile can be live in two
/// instances at once. The credential is the running-table URL's one-time token;
/// the launcher never reads `<DSH_HOME>/.credentials.yaml`.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_plugins(
    state: State<'_, AppState>,
    home_id: String,
    profile: String,
    instance_id: Option<String>,
) -> Result<crate::native::PluginView, String> {
    list_plugins_for(state.inner(), &home_id, &profile, instance_id.as_deref()).await
}

async fn list_plugins_for(
    state: &AppState,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
) -> Result<PluginView, String> {
    list_plugins_via(state, http_transport()?, home_id, profile, instance_id).await
}

async fn list_plugins_via(
    state: &AppState,
    transport: Arc<dyn NativeTransport>,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
) -> Result<PluginView, String> {
    const NOT_RUNNING: &str = "实例未在运行：启动该实例后即可在这里管理它的补丁层。";
    const ADOPTED: &str =
        "该实例由系统接管（DSH 自行重启），没有可用的启动 token，无法管理补丁层。请从启动器重启该实例。";

    let (home, host) = home_and_host(state, home_id)?;

    if let Some(reason) = host_unreachable_note(state, &host) {
        return Ok(PluginView::readonly(
            PluginViewMode::Unreachable,
            reason,
            Vec::new(),
        ));
    }

    let Some(entry) =
        crate::process::running_for_profile(state, home_id, profile, instance_id).await?
    else {
        let bundles = profile_projection(state, &host, &home, profile).await;
        return Ok(PluginView::readonly(
            PluginViewMode::NotRunning,
            NOT_RUNNING,
            bundles,
        ));
    };
    if crate::process::manageable_url(&entry).is_none() {
        let mode = if entry.adopted {
            PluginViewMode::Adopted
        } else {
            PluginViewMode::NotRunning
        };
        let reason = if entry.adopted { ADOPTED } else { NOT_RUNNING };
        let bundles = profile_projection(state, &host, &home, profile).await;
        return Ok(PluginView::readonly(mode, reason, bundles));
    }

    let url = match crate::process::reachable_url_for(state, &entry.id).await {
        Ok(url) => url,
        Err(e) => {
            let bundles = profile_projection(state, &host, &home, profile).await;
            return Ok(PluginView::readonly(
                PluginViewMode::Unreachable,
                e,
                bundles,
            ));
        }
    };
    let client = match NativeClient::connect_with(transport, &url).await {
        Ok(client) => client,
        Err(e) => {
            let bundles = profile_projection(state, &host, &home, profile).await;
            return Ok(PluginView::readonly(
                PluginViewMode::Unreachable,
                e,
                bundles,
            ));
        }
    };
    let bundles = client.list_bundles().await?;
    Ok(PluginView::managed(bundles, &ui_locale(state), &entry.id))
}

fn home_and_host(state: &AppState, home_id: &str) -> Result<(DshHome, Host), String> {
    let cfg = state.config.lock().unwrap();
    let home = cfg
        .homes
        .iter()
        .find(|h| h.id == home_id)
        .cloned()
        .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
    let host = crate::config::resolve_host(&cfg, Some(&home.host_id))?.clone();
    Ok((home, host))
}

fn host_unreachable_note(state: &AppState, host: &Host) -> Option<String> {
    let reason = state.host_unreachable_reason(&host.id)?;
    Some(format!(
        "主机「{}」现在连不上（{reason}）。那台机器上的实例问不到，Profile 文件也读不到；恢复连接后点「重试」。",
        host.display_name
    ))
}

async fn profile_projection(
    state: &AppState,
    host: &Host,
    home: &DshHome,
    profile: &str,
) -> Vec<BundleItem> {
    if host.builtin {
        return offline_bundles_in(&home.path, profile);
    }
    let Ok(dir) = crate::profile::profile_dir(&home.path, profile) else {
        return Vec::new();
    };
    let run_host = match crate::ssh::RemoteHost::of(host, &state.data_dir) {
        Ok(run_host) => run_host,
        Err(e) => {
            crate::log_warn!("读取远端 Profile 文件失败：{e}");
            return Vec::new();
        }
    };
    let files = match crate::remote::profile_packages(
        &*state.host_transport,
        &run_host,
        &dir.to_string_lossy(),
    )
    .await
    {
        Ok(files) => files,
        Err(e) => {
            crate::log_warn!("读取「{}」上的 Profile 文件失败：{e}", host.display_name);
            return Vec::new();
        }
    };
    offline_bundles_from(
        files
            .manifest
            .as_deref()
            .and_then(|raw| std::str::from_utf8(raw).ok()),
        |name| files.installed.get(name).cloned(),
    )
}

pub(crate) fn offline_bundles_from<F>(manifest: Option<&str>, version_of: F) -> Vec<BundleItem>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(raw) = manifest else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };

    let strings = |value: Option<&serde_json::Value>| -> Vec<String> {
        value
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let selected = strings(doc.pointer("/dsh/profile/bundles"));
    let installed: Vec<String> = doc
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|deps| deps.keys().cloned().collect())
        .unwrap_or_default();

    let mut names = selected.clone();
    let mut rest: Vec<String> = installed
        .iter()
        .filter(|name| !selected.contains(name))
        .cloned()
        .collect();
    rest.sort();
    names.extend(rest);

    names
        .into_iter()
        .map(|name| {
            let version = version_of(&name);
            BundleItem::from_profile_files(
                name.clone(),
                selected.contains(&name),
                installed.contains(&name),
                version,
            )
        })
        .collect()
}

pub(crate) fn offline_bundles_in(
    home: &std::path::Path,
    profile: &str,
) -> Vec<crate::native::BundleItem> {
    let Ok(dir) = crate::profile::profile_dir(home, profile) else {
        return Vec::new();
    };
    let manifest = std::fs::read_to_string(dir.join("package.json")).ok();
    offline_bundles_from(manifest.as_deref(), |name| {
        read_installed_version(&dir, name)
    })
}

fn read_installed_version(profile_dir: &std::path::Path, name: &str) -> Option<String> {
    let raw = std::fs::read_to_string(
        profile_dir
            .join("node_modules")
            .join(name)
            .join("package.json"),
    )
    .ok()?;
    let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;
    doc.get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub(crate) fn ui_locale(_state: &AppState) -> String {
    "zh-CN".to_string()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBundleEnabledInput {
    #[serde(alias = "home_id")]
    pub home_id: String,
    pub profile: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    pub name: String,
    pub enabled: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_bundle_enabled(
    state: State<'_, AppState>,
    input: SetBundleEnabledInput,
) -> Result<ChangeOutcome, String> {
    set_bundle_enabled_for(
        state.inner(),
        &input.home_id,
        &input.profile,
        input.instance_id.as_deref(),
        &input.name,
        input.enabled,
    )
    .await
}

pub(crate) async fn set_bundle_enabled_for(
    state: &AppState,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
    name: &str,
    enabled: bool,
) -> Result<ChangeOutcome, String> {
    let client = write_client(state, home_id, profile, instance_id).await?;
    let result = client.set_bundle_enabled(name, enabled).await?;
    Ok(localize_refusal(ChangeOutcome::from_native(
        result,
        &ui_locale(state),
    )))
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeOutcome {
    pub changed: bool,
    pub application: String,
    pub stage: String,
    pub target: String,
    /// The **raw** native `error.code` for a refusal (`changed:false`); logic,
    /// not display — the localized reason rides in `diagnostic`.
    pub error_code: Option<String>,
    pub diagnostic: Option<String>,
    pub warnings: Vec<String>,
}

impl ChangeOutcome {
    pub(crate) fn from_native(r: NativeChangeResult, locale: &str) -> Self {
        let (error_code, diagnostic) = match r.error {
            Some(e) => (
                Some(e.code),
                e.diagnostic.as_ref().and_then(|d| d.pick_opt(locale)),
            ),
            None => (None, None),
        };
        Self {
            changed: r.changed,
            application: r.application,
            stage: r.stage,
            target: r.target,
            error_code,
            diagnostic,
            warnings: r.warnings,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveBundleInput {
    #[serde(alias = "home_id")]
    pub home_id: String,
    pub profile: String,
    #[serde(default)]
    pub instance_id: Option<String>,
    pub name: String,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_bundle(
    state: State<'_, AppState>,
    input: RemoveBundleInput,
) -> Result<ChangeOutcome, String> {
    remove_bundle_for(state.inner(), &input).await
}

pub(crate) async fn remove_bundle_for(
    state: &AppState,
    input: &RemoveBundleInput,
) -> Result<ChangeOutcome, String> {
    let client = write_client(
        state,
        &input.home_id,
        &input.profile,
        input.instance_id.as_deref(),
    )
    .await?;
    let result = client.remove_bundle(&input.name).await?;
    Ok(localize_refusal(ChangeOutcome::from_native(
        result,
        &ui_locale(state),
    )))
}

/// Reads the store a profile's `node_modules` is currently linked from, via
/// the `storeDir` entry pnpm records in `node_modules/.modules.yaml`. Older
/// pnpm wrote YAML (`storeDir: /path`); pnpm 10+ writes JSON, so both
/// layouts are parsed.
fn linked_store_dir(profile_dir: &std::path::Path) -> Option<String> {
    let raw =
        std::fs::read_to_string(profile_dir.join("node_modules").join(".modules.yaml")).ok()?;
    linked_store_document(&raw)
}

pub(crate) fn linked_store_document(raw: &str) -> Option<String> {
    for line in raw.lines() {
        if let Some(v) = line.trim().strip_prefix("storeDir:") {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    if let Ok(doc) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(v) = doc.get("storeDir").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Whether two store paths point at the same store. pnpm records the
/// versioned subdirectory (`<store>/v11`) while the launcher pins the base
/// dir, so a path containing the other as a prefix also counts as a match.
/// Checks whether two store paths match or one is an ancestor of the other.
fn store_paths_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_string();
    let (a, b) = (norm(a), norm(b));
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

pub(crate) fn profile_store_target(
    profile_dir: &std::path::Path,
    launcher_store: &std::path::Path,
) -> std::path::PathBuf {
    store_target_from(linked_store_dir(profile_dir).as_deref(), launcher_store)
}

pub(crate) fn store_target_from(
    linked: Option<&str>,
    launcher_store: &std::path::Path,
) -> std::path::PathBuf {
    match linked {
        Some(linked) if !store_paths_match(linked, &launcher_store.to_string_lossy()) => {
            std::path::PathBuf::from(linked)
        }
        _ => launcher_store.to_path_buf(),
    }
}

fn yaml_double_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn ensure_profile_store_pin(
    dir: &std::path::Path,
    store_dir: &std::path::Path,
) -> Result<(), String> {
    let path = dir.join("pnpm-workspace.yaml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("读取 pnpm-workspace.yaml 失败: {e}")),
    };
    let Some(out) = store_pin_document(raw.as_deref(), store_dir) else {
        return Ok(());
    };

    std::fs::create_dir_all(dir).map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    std::fs::write(&path, out).map_err(|e| format!("写入 pnpm-workspace.yaml 失败: {e}"))
}

pub(crate) fn store_pin_document(raw: Option<&str>, store_dir: &std::path::Path) -> Option<String> {
    const FRESH: &str = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n";
    let raw = raw.unwrap_or(FRESH);
    let pinned = format!(
        "storeDir: {}",
        yaml_double_quoted(&store_dir.to_string_lossy())
    );
    let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
    let mut found = false;
    let mut changed = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if line.len() != trimmed.len() || trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.starts_with("storeDir:") {
            continue;
        }
        found = true;
        if trimmed != pinned {
            *line = pinned.clone();
            changed = true;
        }
    }
    if !found {
        if !lines.is_empty() && !lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.push(String::new());
        }
        lines.push(pinned);
        changed = true;
    }
    if !changed {
        return None;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Some(out)
}

pub(crate) fn ensure_profile_npmrc(dir: &std::path::Path) -> Result<(), String> {
    let path = dir.join(".npmrc");
    let raw = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| format!("读取 .npmrc 失败: {e}"))?
    } else {
        String::new()
    };
    let Some(out) = npmrc_document(&raw) else {
        return Ok(());
    };
    std::fs::write(&path, out).map_err(|e| format!("写入 .npmrc 失败: {e}"))
}

pub(crate) fn npmrc_document(raw: &str) -> Option<String> {
    const KEY: &str = "auto-install-peers";
    let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
    let mut found = false;
    let mut changed = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        if k.trim() != KEY {
            continue;
        }
        found = true;
        if v.trim() != "false" {
            *line = format!("{KEY}=false");
            changed = true;
        }
    }
    if !found {
        if !lines.is_empty() && !lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.push(String::new());
        }
        lines.push("# DSH: core packages come from the CLI dependency tree;".to_string());
        lines.push("# a profile must never resolve its own copy.".to_string());
        lines.push(format!("{KEY}=false"));
        changed = true;
    }
    if !changed {
        return None;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Some(out)
}

pub(crate) fn ensure_build_scripts_allowed(
    dir: &std::path::Path,
    approve: &[String],
) -> Result<(), String> {
    let ws_manifest = dir.join("pnpm-workspace.yaml");
    let raw = if ws_manifest.exists() {
        std::fs::read_to_string(&ws_manifest)
            .map_err(|e| format!("读取 pnpm-workspace.yaml 失败: {e}"))?
    } else {
        String::new()
    };
    let manifest = build_scripts_manifest(&raw, approve);
    if let Some(manifest) = manifest {
        std::fs::write(&ws_manifest, manifest)
            .map_err(|e| format!("写入 pnpm-workspace.yaml 失败: {e}"))?;
    }
    Ok(())
}

pub(crate) fn build_scripts_manifest(raw: &str, approve: &[String]) -> Option<String> {
    // Base document: `packages` is required for pnpm to treat the dir as a
    // workspace (needed for allowBuilds to be read from this file).
    let mut lines: Vec<String> = if raw.trim().is_empty() {
        vec!["packages:".to_string(), "  - .".to_string()]
    } else {
        raw.lines().map(|l| l.to_string()).collect()
    };

    let mut changed = false;
    for line in lines.iter_mut() {
        if line.contains("set this to true or false") {
            *line = line.replace("set this to true or false", "true");
            changed = true;
        }
    }

    let joined = lines.join("\n");
    if !joined.contains("onlyBuiltDependencies") {
        lines.push(String::new());
        lines.push("onlyBuiltDependencies:".to_string());
        lines.push("  - '*'".to_string());
        changed = true;
    }

    // 3. Ensure an `allowBuilds:` section exists so pnpm 11 has somewhere to
    //    record newly-ignored builds (it auto-appends entries on failure).
    if !lines
        .iter()
        .any(|l| l.trim_start().starts_with("allowBuilds:"))
    {
        lines.push(String::new());
        lines.push("allowBuilds:".to_string());
        changed = true;
    }

    let named = allow_builds_names(&lines);
    let missing: Vec<&String> = approve
        .iter()
        .filter(|name| !named.contains(&name.as_str()))
        .collect();
    if !missing.is_empty() {
        let at = allow_builds_insert_at(&lines);
        for (offset, name) in missing.iter().enumerate() {
            lines.insert(at + offset, format!("  {}: true", allow_builds_key(name)));
        }
        changed = true;
    }

    if !changed {
        return None;
    }
    if !lines.join("\n").ends_with('\n') {
        lines.push(String::new());
    }
    Some(lines.join("\n"))
}

fn allow_builds_insert_at(lines: &[String]) -> usize {
    let header = lines
        .iter()
        .position(|line| line.trim_start().starts_with("allowBuilds:"))
        .unwrap_or(lines.len());
    let mut at = lines
        .iter()
        .skip(header + 1)
        .position(|line| !line.trim().is_empty() && !line.starts_with([' ', '\t']))
        .map(|offset| header + 1 + offset)
        .unwrap_or(lines.len());
    while at > header + 1 && lines[at - 1].trim().is_empty() {
        at -= 1;
    }
    at
}

fn allow_builds_names(lines: &[String]) -> Vec<&str> {
    let header = lines
        .iter()
        .position(|line| line.trim_start().starts_with("allowBuilds:"));
    let Some(header) = header else {
        return Vec::new();
    };
    lines[header + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.starts_with([' ', '\t']))
        .filter_map(|line| line.split_once(':'))
        .map(|(key, _)| key.trim().trim_matches(['\'', '"']))
        .collect()
}

/// One package name as a YAML mapping key. A scoped name has to be quoted: `@`
/// may not begin a plain scalar.
fn allow_builds_key(name: &str) -> String {
    let plain = name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'));
    if plain {
        return name.to_string();
    }
    yaml_double_quoted(name)
}

pub(crate) async fn write_client(
    state: &AppState,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
) -> Result<crate::native::NativeClient, String> {
    write_client_via(state, http_transport()?, home_id, profile, instance_id).await
}

pub(crate) async fn write_client_via(
    state: &AppState,
    transport: Arc<dyn NativeTransport>,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
) -> Result<crate::native::NativeClient, String> {
    let (_, host) = home_and_host(state, home_id)?;
    if let Some(reason) = host_unreachable_note(state, &host) {
        return Err(reason);
    }
    let Some(entry) =
        crate::process::running_for_profile(state, home_id, profile, instance_id).await?
    else {
        return Err("实例未在运行，请先启动实例".to_string());
    };
    if crate::process::manageable_url(&entry).is_none() {
        return Err(if entry.adopted {
            "该实例由系统接管，无法管理插件".to_string()
        } else {
            "实例未在运行，请先启动实例".to_string()
        });
    }
    let url = crate::process::reachable_url_for(state, &entry.id).await?;
    crate::native::NativeClient::connect_with(transport, &url).await
}

/// Translates a native `error.code` refusal into the reason the page shows
/// verbatim (design §4d). This is the **one** place codes become Chinese; the
/// UI never maps them. An unlisted code is surfaced as-is, and an attached
/// `diagnostic` is appended in parentheses.
pub(crate) fn refusal_reason(code: &str, diagnostic: Option<&str>) -> String {
    let base = match code {
        "management-required" | "unaddressable" => "该条目由 DSH 自身管理，不能在这里改动",
        "not-removable" => "该 Bundle 随 DSH 发布，只能停用不能删除",
        "unknown-plugin" => "实例里找不到这个插件",
        "" => "实例未说明原因",
        other => other,
    };
    match diagnostic.filter(|d| !d.trim().is_empty()) {
        Some(d) => format!("{base}（{d}）"),
        None => base.to_string(),
    }
}

/// Turns a refused write into the exact Chinese reason the page shows:
/// `errorCode` stays for logic, and the localized reason rides in `diagnostic`
/// (the frozen [`ChangeOutcome`] has no separate
/// field) — so the UI shows it verbatim instead of guessing (design §4d).
///
/// Only a bare refusal (`changed:false` with a code) is rewritten; a
/// `changed:true` + `failed` keeps the instance's own diagnostic (design §4b).
pub(crate) fn localize_refusal(outcome: ChangeOutcome) -> ChangeOutcome {
    if outcome.changed {
        return outcome;
    }
    let Some(code) = outcome.error_code.clone() else {
        return outcome;
    };
    let reason = refusal_reason(&code, outcome.diagnostic.as_deref());
    ChangeOutcome {
        diagnostic: Some(reason),
        ..outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_paths_match_handles_versioned_subdir_and_slashes() {
        let base = "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store";
        assert!(store_paths_match(&format!("{base}/v11"), base));
        assert!(store_paths_match(
            "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store/v11/",
            base
        ));
        assert!(!store_paths_match("/Users/x/Library/pnpm/store/v11", base));
    }

    #[test]
    fn linked_store_dir_reads_modules_yaml() {
        let dir = std::env::temp_dir().join(format!("dsh-test-modules-{}", uuid::Uuid::new_v4()));
        let nm = dir.join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(
            nm.join(".modules.yaml"),
            "hoist: true\nstoreDir: C:\\Users\\x\\AppData\\Local\\pnpm\\store\\v11\nvirtualStoreDir: ...\n",
        )
        .unwrap();
        assert_eq!(
            linked_store_dir(&dir).as_deref(),
            Some("C:\\Users\\x\\AppData\\Local\\pnpm\\store\\v11")
        );
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(linked_store_dir(&dir), None);
    }

    #[test]
    fn linked_store_dir_reads_json_modules_yaml() {
        let dir = std::env::temp_dir().join(format!("dsh-test-modules-{}", uuid::Uuid::new_v4()));
        let nm = dir.join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(
            nm.join(".modules.yaml"),
            r#"{
  "hoistPattern": ["*"],
  "packageManager": "pnpm@11.21.0",
  "storeDir": "/Users/x/Library/pnpm/store/v11",
  "virtualStoreDir": ".pnpm",
  "layoutVersion": 5
}
"#,
        )
        .unwrap();
        assert_eq!(
            linked_store_dir(&dir).as_deref(),
            Some("/Users/x/Library/pnpm/store/v11")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_profile_store_pin_writes_workspace_setting() {
        let dir = std::env::temp_dir().join(format!("dsh-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ws = dir.join("pnpm-workspace.yaml");
        let store = "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store";

        std::fs::write(
            &ws,
            "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n",
        )
        .unwrap();
        ensure_profile_store_pin(&dir, std::path::Path::new(store)).unwrap();
        let pinned = std::fs::read_to_string(&ws).unwrap();
        assert!(
            pinned.contains(&format!("storeDir: \"{store}\"")),
            "pinned: {pinned}"
        );
        assert!(pinned.contains("nodeLinker: hoisted"), "pinned: {pinned}");
        assert!(
            pinned.contains("autoInstallPeers: false"),
            "pinned: {pinned}"
        );

        ensure_profile_store_pin(&dir, std::path::Path::new(store)).unwrap();
        assert_eq!(std::fs::read_to_string(&ws).unwrap(), pinned);

        let moved = "/Users/x/other store/.pnpm-store";
        ensure_profile_store_pin(&dir, std::path::Path::new(moved)).unwrap();
        let repinned = std::fs::read_to_string(&ws).unwrap();
        assert_eq!(
            repinned.matches("storeDir:").count(),
            1,
            "repinned: {repinned}"
        );
        assert!(
            repinned.contains(&format!("storeDir: \"{moved}\"")),
            "repinned: {repinned}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_profile_store_pin_keeps_dsh_workspace_defaults_on_fresh_dir() {
        let dir = std::env::temp_dir().join(format!("dsh-store-test-{}", uuid::Uuid::new_v4()));
        let store = "/Users/x/.pnpm-store";
        ensure_profile_store_pin(&dir, std::path::Path::new(store)).unwrap();
        let fresh = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(fresh.contains("packages:"), "fresh: {fresh}");
        assert!(fresh.contains("nodeLinker: hoisted"), "fresh: {fresh}");
        assert!(fresh.contains("autoInstallPeers: false"), "fresh: {fresh}");
        assert!(
            fresh.contains(&format!("storeDir: \"{store}\"")),
            "fresh: {fresh}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_profile_store_pin_ignores_nested_and_commented_keys() {
        let dir = std::env::temp_dir().join(format!("dsh-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ws = dir.join("pnpm-workspace.yaml");
        std::fs::write(
            &ws,
            "packages:\n  - .\n# storeDir: /old\nfoo:\n  storeDir: /nested\n",
        )
        .unwrap();
        ensure_profile_store_pin(&dir, std::path::Path::new("/Users/x/.pnpm-store")).unwrap();
        let out = std::fs::read_to_string(&ws).unwrap();
        assert!(out.contains("# storeDir: /old"), "out: {out}");
        assert!(out.contains("  storeDir: /nested"), "out: {out}");
        assert!(
            out.contains("storeDir: \"/Users/x/.pnpm-store\""),
            "out: {out}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn profile_store_target_follows_the_linked_store() {
        let dir = std::env::temp_dir().join(format!("dsh-store-test-{}", uuid::Uuid::new_v4()));
        let nm = dir.join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        let launcher = "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store";

        assert_eq!(
            profile_store_target(&dir, std::path::Path::new(launcher)),
            std::path::PathBuf::from(launcher)
        );

        std::fs::write(
            nm.join(".modules.yaml"),
            format!("{{\"storeDir\": \"{launcher}/v11\"}}\n"),
        )
        .unwrap();
        assert_eq!(
            profile_store_target(&dir, std::path::Path::new(launcher)),
            std::path::PathBuf::from(launcher)
        );

        std::fs::write(
            nm.join(".modules.yaml"),
            "{\"storeDir\": \"/Users/x/Library/pnpm/store/v11\"}\n",
        )
        .unwrap();
        assert_eq!(
            profile_store_target(&dir, std::path::Path::new(launcher)),
            std::path::PathBuf::from("/Users/x/Library/pnpm/store/v11")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn yaml_double_quoted_escapes_backslash_and_quote() {
        assert_eq!(yaml_double_quoted("/a b/c"), "\"/a b/c\"");
        assert_eq!(yaml_double_quoted("C:\\store"), "\"C:\\\\store\"");
        assert_eq!(yaml_double_quoted("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn ensure_build_scripts_allowed_converts_placeholders_and_adds_sections() {
        let dir = std::env::temp_dir().join(format!("dsh-plugins-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        ensure_build_scripts_allowed(&dir, &[]).unwrap();
        let fresh = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(fresh.contains("packages:"), "fresh: {fresh}");
        assert!(fresh.contains("onlyBuiltDependencies"), "fresh: {fresh}");
        assert!(fresh.contains("allowBuilds:"), "fresh: {fresh}");

        std::fs::write(
            dir.join("pnpm-workspace.yaml"),
            "packages:\n  - .\nallowBuilds:\n  node-pty: set this to true or false\n",
        )
        .unwrap();
        ensure_build_scripts_allowed(&dir, &[]).unwrap();
        let fixed = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(fixed.contains("node-pty: true"), "fixed: {fixed}");
        assert!(
            !fixed.contains("set this to true or false"),
            "fixed: {fixed}"
        );
        assert!(fixed.contains("onlyBuiltDependencies"), "fixed: {fixed}");

        let before = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        ensure_build_scripts_allowed(&dir, &[]).unwrap();
        let after = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert_eq!(before, after, "must be idempotent");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_scripts_manifest_names_the_packages_pnpm_refused() {
        let approve = [
            "koffi".to_string(),
            "@deepseek-ai/dsh-subprocess-local".to_string(),
        ];
        let document = build_scripts_manifest("", &approve).expect("空文件要写出清单");

        assert!(document.contains("allowBuilds:"), "{document}");
        assert!(
            document.contains("  koffi: true"),
            "普通包名照原样写：{document}"
        );
        assert!(
            document.contains("  \"@deepseek-ai/dsh-subprocess-local\": true"),
            "@ 开头的名字在 YAML 里要加引号：{document}"
        );
        assert!(
            document.contains("onlyBuiltDependencies:\n  - '*'"),
            "pnpm 10 只认这一段，它要继续留着：{document}"
        );

        assert!(
            build_scripts_manifest(&document, &approve).is_none(),
            "已批准过的名字不该再写一遍"
        );

        let more = ["koffi".to_string(), "protobufjs".to_string()];
        let second = build_scripts_manifest(&document, &more).expect("新名字要写进去");
        assert!(second.contains("  protobufjs: true"), "{second}");
        assert_eq!(second.matches("koffi: true").count(), 1, "{second}");
        assert!(
            second.contains("allowBuilds:\n  koffi: true"),
            "批准项要落在 allowBuilds 段里：{second}"
        );
    }

    #[test]
    fn ensure_profile_npmrc_pins_auto_install_peers_false() {
        let dir = std::env::temp_dir().join(format!("dsh-npmrc-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let npmrc = dir.join(".npmrc");

        ensure_profile_npmrc(&dir).unwrap();
        let fresh = std::fs::read_to_string(&npmrc).unwrap();
        assert!(fresh.contains("auto-install-peers=false"), "fresh: {fresh}");

        ensure_profile_npmrc(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&npmrc).unwrap(), fresh);

        std::fs::write(
            &npmrc,
            "registry=https://example.com/\nauto-install-peers=true\n",
        )
        .unwrap();
        ensure_profile_npmrc(&dir).unwrap();
        let fixed = std::fs::read_to_string(&npmrc).unwrap();
        assert!(fixed.contains("auto-install-peers=false"), "fixed: {fixed}");
        assert!(!fixed.contains("auto-install-peers=true"), "fixed: {fixed}");
        assert!(
            fixed.contains("registry=https://example.com/"),
            "other keys must survive: {fixed}"
        );

        std::fs::write(&npmrc, "# auto-install-peers=true\n").unwrap();
        ensure_profile_npmrc(&dir).unwrap();
        let commented = std::fs::read_to_string(&npmrc).unwrap();
        assert!(
            commented.contains("\nauto-install-peers=false"),
            "commented: {commented}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    fn app_state(
        homes: Vec<crate::config::DshHome>,
        instances: Vec<crate::config::DshInstance>,
    ) -> AppState {
        app_state_with(
            vec![crate::config::Host::local()],
            homes,
            instances,
            crate::remote::testing::idle(),
        )
    }

    fn app_state_with(
        hosts: Vec<crate::config::Host>,
        homes: Vec<crate::config::DshHome>,
        instances: Vec<crate::config::DshInstance>,
        transport: Arc<dyn crate::ssh::HostTransport>,
    ) -> AppState {
        AppState {
            config_path: std::path::PathBuf::new(),
            data_dir: std::path::PathBuf::new(),
            config: std::sync::Mutex::new(crate::config::Config {
                homes,
                versions: Vec::new(),
                instances,
                settings: Default::default(),
                hosts,
            }),
            running: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            tasks: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            host_transport: transport,
            host_statuses: std::sync::Mutex::new(std::collections::HashMap::new()),
            remote_scan: std::sync::Mutex::new(std::collections::HashSet::new()),
            forwards: Default::default(),
        }
    }

    fn scratch_home(tag: &str) -> std::path::PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri has a parent")
            .join("tmp");
        let dir = root.join(format!("t5-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn home_with_path(id: &str, path: &std::path::Path) -> crate::config::DshHome {
        crate::config::DshHome {
            id: id.to_string(),
            name: id.to_string(),
            path: path.to_path_buf(),
            host_id: crate::config::LOCAL_HOST_ID.to_string(),
        }
    }

    fn instance_of(id: &str, home_id: &str) -> crate::config::DshInstance {
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

    fn profile_json(deps: &[&str], bundles: &[&str]) -> String {
        let deps: serde_json::Map<String, serde_json::Value> = deps
            .iter()
            .map(|d| (d.to_string(), serde_json::json!("^1.0.0")))
            .collect();
        serde_json::json!({
            "private": true,
            "dependencies": deps,
            "dsh": { "profile": { "bundles": bundles } },
        })
        .to_string()
    }

    fn seeded_home(files: &[(&str, &str)], profile: &str) -> std::path::PathBuf {
        let home = scratch_home("home");
        let dir = crate::profile::profile_dir(&home, profile).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            std::fs::write(dir.join(name), content).unwrap();
        }
        home
    }

    #[tokio::test]
    async fn list_plugins_falls_back_to_the_profile_files_when_not_running() {
        let home = seeded_home(
            &[
                (
                    "package.json",
                    &profile_json(
                        &["@dsh-plugin/dsh-auxiliary", "@deepseek-ai/dsh-base"],
                        &["@deepseek-ai/dsh-base", "@dsh-plugin/dsh-auxiliary"],
                    ),
                ),
                (
                    "cordis.patch.yml",
                    "[ { id: dsh-auxiliary, disabled: true }, { id: dsh-base, disabled: false } ]\n",
                ),
            ],
            "web",
        );
        let state = app_state(
            vec![home_with_path("h1", &home)],
            vec![instance_of("i1", "h1")],
        );

        let view = list_plugins_for(&state, "h1", "web", None).await.unwrap();

        assert_eq!(view.mode, crate::native::PluginViewMode::NotRunning);
        assert!(view.instance_id.is_none(), "no instance answered");
        let reason = view.reason.expect("read-only reason");
        assert!(reason.contains("启动该实例"), "reason: {reason}");

        let names: Vec<&str> = view.bundles.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["@deepseek-ai/dsh-base", "@dsh-plugin/dsh-auxiliary"]
        );
        assert!(view.bundles.iter().all(|b| b.enabled && b.installed));
        assert!(view.bundles.iter().all(|b| b.row_count == 0
            && !b.removable
            && b.version.is_none()
            && b.availability.is_none()
            && b.error_code.is_none()));
    }

    #[tokio::test]
    async fn offline_bundles_read_only_what_the_profile_declares() {
        let home = seeded_home(
            &[(
                "package.json",
                &profile_json(
                    &["@deepseek-ai/dsh-base", "ds-bot"],
                    &["@deepseek-ai/dsh-base"],
                ),
            )],
            "web",
        );
        let dir = crate::profile::profile_dir(&home, "web").unwrap();
        std::fs::create_dir_all(dir.join("node_modules").join("@deepseek-ai/dsh-base")).unwrap();
        std::fs::write(
            dir.join("node_modules")
                .join("@deepseek-ai/dsh-base")
                .join("package.json"),
            r#"{"name":"@deepseek-ai/dsh-base","version":"0.2.1-alpha.2"}"#,
        )
        .unwrap();

        let bundles = offline_bundles_in(&home, "web");
        let by_name = |name: &str| bundles.iter().find(|b| b.name == name).unwrap().clone();
        let base = by_name("@deepseek-ai/dsh-base");
        assert!(base.enabled && base.installed);
        assert_eq!(base.version.as_deref(), Some("0.2.1-alpha.2"));
        let bot = by_name("ds-bot");
        assert!(!bot.enabled, "installed but not selected = unmounted");
        assert!(bot.installed);
        assert_eq!(bundles.len(), 2);

        assert!(offline_bundles_in(&home, "nope").is_empty());
        std::fs::write(dir.join("package.json"), "not json").unwrap();
        assert!(offline_bundles_in(&home, "web").is_empty());
    }

    #[tokio::test]
    async fn list_plugins_marks_adopted_instances_readonly() {
        let home = scratch_home("adopted");
        let state = app_state(
            vec![home_with_path("h1", &home)],
            vec![instance_of("i1", "h1")],
        );
        {
            let mut running = state.running.lock().await;
            running.insert(
                "i1".to_string(),
                crate::process::RunningInstance {
                    kill: std::sync::Arc::new(tokio::sync::Notify::new()),
                    profile: "web".to_string(),
                    url: Some("http://127.0.0.1:3089".to_string()),
                    adopted: Some(crate::process::Adopted {
                        pid: 1,
                        port: 3089,
                        host: "127.0.0.1".to_string(),
                    }),
                    remote: None,
                },
            );
        }
        let view = list_plugins_for(&state, "h1", "web", None).await.unwrap();
        assert_eq!(view.mode, crate::native::PluginViewMode::Adopted);
        assert!(view.bundles.is_empty());
        let adopted_reason = view.reason.expect("adopted reason");
        assert!(
            adopted_reason.contains("接管"),
            "adopted reason must name the adoption: {adopted_reason}"
        );

        let not_running = crate::native::PluginView::readonly(
            crate::native::PluginViewMode::NotRunning,
            "实例未运行",
            Vec::new(),
        );
        assert_ne!(Some(adopted_reason), not_running.reason);
    }

    #[tokio::test]
    async fn an_adopted_instance_with_a_token_is_still_addressed() {
        let home = scratch_home("adopted-token");
        let state = app_state(
            vec![home_with_path("h1", &home)],
            vec![instance_of("i1", "h1")],
        );
        {
            let mut running = state.running.lock().await;
            running.insert(
                "i1".to_string(),
                crate::process::RunningInstance {
                    kill: std::sync::Arc::new(tokio::sync::Notify::new()),
                    profile: "web".to_string(),
                    url: Some("http://127.0.0.1:1/?token=remembered".to_string()),
                    adopted: Some(crate::process::Adopted {
                        pid: 1,
                        port: 1,
                        host: "127.0.0.1".to_string(),
                    }),
                    remote: None,
                },
            );
        }
        let err = match write_client(&state, "h1", "web", None).await {
            Ok(_) => panic!("no instance is reachable in this test"),
            Err(e) => e,
        };
        assert!(
            !err.contains("接管"),
            "an adopted instance we hold a token for must be attempted, got: {err}"
        );
    }

    #[tokio::test]
    async fn set_bundle_enabled_refuses_without_a_running_instance() {
        let state = app_state(
            vec![crate::config::DshHome {
                id: "h1".to_string(),
                name: "h1".to_string(),
                path: std::path::PathBuf::from("/tmp/home"),
                host_id: crate::config::LOCAL_HOST_ID.to_string(),
            }],
            vec![crate::config::DshInstance {
                id: "i1".to_string(),
                name: "i1".to_string(),
                version_id: "v1".to_string(),
                home_id: "h1".to_string(),
                host_id: crate::config::LOCAL_HOST_ID.to_string(),
                env_overrides: Default::default(),
                default_profile: None,
                last_profile: None,
                icon: None,
                port: None,
            }],
        );
        let err = set_bundle_enabled_for(&state, "h1", "web", None, "@deepseek-ai/dsh-llm", false)
            .await
            .unwrap_err();
        assert!(err.contains("未在运行"), "err: {err}");
    }

    #[tokio::test]
    async fn set_bundle_enabled_refuses_for_an_adopted_instance() {
        let state = app_state(
            vec![crate::config::DshHome {
                id: "h1".to_string(),
                name: "h1".to_string(),
                path: std::path::PathBuf::from("/tmp/home"),
                host_id: crate::config::LOCAL_HOST_ID.to_string(),
            }],
            vec![crate::config::DshInstance {
                id: "i1".to_string(),
                name: "i1".to_string(),
                version_id: "v1".to_string(),
                home_id: "h1".to_string(),
                host_id: crate::config::LOCAL_HOST_ID.to_string(),
                env_overrides: Default::default(),
                default_profile: None,
                last_profile: None,
                icon: None,
                port: None,
            }],
        );
        {
            let mut running = state.running.lock().await;
            running.insert(
                "i1".to_string(),
                crate::process::RunningInstance {
                    kill: std::sync::Arc::new(tokio::sync::Notify::new()),
                    profile: "web".to_string(),
                    url: Some("http://127.0.0.1:3089".to_string()),
                    adopted: Some(crate::process::Adopted {
                        pid: 1,
                        port: 3089,
                        host: "127.0.0.1".to_string(),
                    }),
                    remote: None,
                },
            );
        }
        let err = set_bundle_enabled_for(&state, "h1", "web", None, "@deepseek-ai/dsh-llm", false)
            .await
            .unwrap_err();
        assert!(err.contains("接管"), "err: {err}");
    }

    fn one_home() -> Vec<crate::config::DshHome> {
        vec![crate::config::DshHome {
            id: "h1".to_string(),
            name: "h1".to_string(),
            path: std::path::PathBuf::from("/tmp/home"),
            host_id: crate::config::LOCAL_HOST_ID.to_string(),
        }]
    }

    fn one_instance() -> Vec<crate::config::DshInstance> {
        vec![crate::config::DshInstance {
            id: "i1".to_string(),
            name: "i1".to_string(),
            version_id: "v1".to_string(),
            home_id: "h1".to_string(),
            host_id: crate::config::LOCAL_HOST_ID.to_string(),
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        }]
    }

    #[test]
    fn refusal_reason_translates_the_management_codes() {
        assert_eq!(
            refusal_reason("management-required", None),
            "该条目由 DSH 自身管理，不能在这里改动"
        );
        assert_eq!(
            refusal_reason("unaddressable", None),
            "该条目由 DSH 自身管理，不能在这里改动"
        );
        assert_eq!(
            refusal_reason("not-removable", None),
            "该 Bundle 随 DSH 发布，只能停用不能删除"
        );
        assert_eq!(
            refusal_reason("unknown-plugin", None),
            "实例里找不到这个插件"
        );
        assert_eq!(refusal_reason("operation-error", None), "operation-error");
        assert_eq!(refusal_reason("", None), "实例未说明原因");
        assert_eq!(
            refusal_reason("management-required", Some("managed by DSH")),
            "该条目由 DSH 自身管理，不能在这里改动（managed by DSH）"
        );
    }

    #[test]
    fn change_outcome_projects_the_error_and_serializes_camel_case() {
        let native: crate::native::NativeChangeResult = serde_json::from_value(serde_json::json!({
            "changed": false,
            "application": "failed",
            "stage": "enable",
            "target": "include:llm",
            "error": { "code": "unaddressable", "diagnostic": "root group" },
            "warnings": [],
        }))
        .unwrap();
        let outcome = ChangeOutcome::from_native(native, "zh-CN");
        assert_eq!(outcome.error_code.as_deref(), Some("unaddressable"));
        assert_eq!(outcome.diagnostic.as_deref(), Some("root group"));

        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["errorCode"], "unaddressable");
        assert_eq!(json["application"], "failed");
        assert_eq!(json["stage"], "enable");
        assert_eq!(json["target"], "include:llm");
        assert_eq!(json["warnings"], serde_json::json!([]));
    }

    #[test]
    fn write_commands_return_a_raw_code_and_a_localized_diagnostic() {
        let refusal = |code: &str| -> NativeChangeResult {
            serde_json::from_value(serde_json::json!({
                "changed": false,
                "application": "failed",
                "stage": "enable",
                "target": "ds-bot",
                "error": { "code": code },
                "warnings": [],
            }))
            .unwrap()
        };

        let bundle = localize_refusal(ChangeOutcome::from_native(
            refusal("management-required"),
            "zh-CN",
        ));
        assert_eq!(bundle.error_code.as_deref(), Some("management-required"));
        assert_eq!(
            bundle.diagnostic.as_deref(),
            Some("该条目由 DSH 自身管理，不能在这里改动")
        );

        let removed = localize_refusal(ChangeOutcome::from_native(
            refusal("not-removable"),
            "zh-CN",
        ));
        assert_eq!(removed.error_code.as_deref(), Some("not-removable"));
        assert_eq!(
            removed.diagnostic.as_deref(),
            Some("该 Bundle 随 DSH 发布，只能停用不能删除")
        );

        let entry = localize_refusal(ChangeOutcome::from_native(
            refusal("unknown-plugin"),
            "zh-CN",
        ));
        assert_eq!(entry.error_code.as_deref(), Some("unknown-plugin"));
        assert_eq!(entry.diagnostic.as_deref(), Some("实例里找不到这个插件"));

        let bare = localize_refusal(ChangeOutcome::from_native(refusal(""), "zh-CN"));
        assert_eq!(bare.error_code.as_deref(), Some(""));
        assert_eq!(bare.diagnostic.as_deref(), Some("实例未说明原因"));
    }

    #[test]
    fn localize_refusal_folds_the_reason_into_the_outcome() {
        let refused = ChangeOutcome {
            changed: false,
            application: "failed".to_string(),
            stage: "enable".to_string(),
            target: "include:llm".to_string(),
            error_code: Some("management-required".to_string()),
            diagnostic: None,
            warnings: vec![],
        };
        let localized = localize_refusal(refused);
        assert_eq!(
            localized.diagnostic.as_deref(),
            Some("该条目由 DSH 自身管理，不能在这里改动")
        );
        assert_eq!(localized.error_code.as_deref(), Some("management-required"));

        let failed = ChangeOutcome {
            changed: true,
            application: "failed".to_string(),
            stage: "enable".to_string(),
            target: "include:llm".to_string(),
            error_code: Some("operation-error".to_string()),
            diagnostic: Some("port in use".to_string()),
            warnings: vec![],
        };
        let localized = localize_refusal(failed);
        assert_eq!(localized.diagnostic.as_deref(), Some("port in use"));
    }

    fn remove_input(name: &str) -> RemoveBundleInput {
        RemoveBundleInput {
            home_id: "h1".to_string(),
            profile: "web".to_string(),
            instance_id: None,
            name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn remove_bundle_refuses_without_a_running_instance() {
        let state = app_state(one_home(), one_instance());
        let err = remove_bundle_for(&state, &remove_input("ds-bot"))
            .await
            .unwrap_err();
        assert!(err.contains("未在运行"), "err: {err}");
    }

    #[tokio::test]
    async fn remove_bundle_refuses_for_an_adopted_instance() {
        let state = app_state(one_home(), one_instance());
        {
            let mut running = state.running.lock().await;
            running.insert(
                "i1".to_string(),
                crate::process::RunningInstance {
                    kill: std::sync::Arc::new(tokio::sync::Notify::new()),
                    profile: "web".to_string(),
                    url: Some("http://127.0.0.1:3089".to_string()),
                    adopted: Some(crate::process::Adopted {
                        pid: 1,
                        port: 3089,
                        host: "127.0.0.1".to_string(),
                    }),
                    remote: None,
                },
            );
        }
        let err = remove_bundle_for(&state, &remove_input("ds-bot"))
            .await
            .unwrap_err();
        assert!(err.contains("接管"), "err: {err}");
    }

    use crate::native::testing::{
        ok_response, FakeTransport as PluginServer, COOKIE, INSTANCE_URL,
    };
    use crate::remote::testing::{FakeAnswer as HostAnswer, FakeCall, FakeTransport as HostServer};

    const TARGET: &str = "zjq_aliyun";
    const TOKEN: &str = "JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";
    const REMOTE_DATA: &str = "/home/zhou/.local/share/dsh-launcher";
    const REMOTE_HOME: &str = "/home/zhou/.local/share/dsh-launcher/homes/1.2.3";

    fn remote_host() -> crate::config::Host {
        crate::config::Host {
            id: "host-lab".to_string(),
            display_name: "服务器".to_string(),
            ssh_target: TARGET.to_string(),
            registry: None,
            extra_env: Default::default(),
            data_dir: Some(std::path::PathBuf::from(REMOTE_DATA)),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    fn remote_home() -> crate::config::DshHome {
        crate::config::DshHome {
            id: "h2".to_string(),
            name: "h2".to_string(),
            path: std::path::PathBuf::from(REMOTE_HOME),
            host_id: "host-lab".to_string(),
        }
    }

    fn remote_instance(id: &str) -> crate::config::DshInstance {
        crate::config::DshInstance {
            id: id.to_string(),
            name: id.to_string(),
            version_id: "v1".to_string(),
            home_id: "h2".to_string(),
            host_id: "host-lab".to_string(),
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        }
    }

    fn running_entry(
        profile: &str,
        url: String,
        adopted: bool,
        remote: Option<crate::process::RemoteRun>,
    ) -> crate::process::RunningInstance {
        crate::process::RunningInstance {
            kill: Arc::new(tokio::sync::Notify::new()),
            profile: profile.to_string(),
            url: Some(url),
            adopted: adopted.then(|| crate::process::Adopted {
                pid: 4242,
                port: 3080,
                host: "192.0.2.7".to_string(),
            }),
            remote,
        }
    }

    fn remote_run(host: &crate::config::Host, remote_url: &str) -> crate::process::RemoteRun {
        crate::process::RemoteRun {
            host_id: host.id.clone(),
            host: crate::ssh::RemoteHost::of(host, std::path::Path::new("/launcher")).unwrap(),
            remote_url: Some(remote_url.to_string()),
            pid: 4242,
            started_at: Some(11),
            data_dir: std::path::PathBuf::from(REMOTE_DATA),
            log_path: format!("{REMOTE_DATA}/logs/i2.log"),
        }
    }

    fn remote_url() -> String {
        format!("http://192.0.2.7:3080/?token={TOKEN}")
    }

    fn stale_local_url() -> String {
        format!("http://127.0.0.1:9999/?token={TOKEN}")
    }

    async fn put_running(state: &AppState, id: &str, entry: crate::process::RunningInstance) {
        state.running.lock().await.insert(id.to_string(), entry);
    }

    fn forwarded(fake: &HostServer) -> Vec<crate::ssh::PortForward> {
        fake.calls()
            .into_iter()
            .filter_map(|call| match call {
                FakeCall::OpenForward { forward, .. } => Some(forward),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_remote_instance_lists_its_layers_over_the_forwarded_address() {
        let host = remote_host();
        let hosts = HostServer::new().answering(HostAnswer::Done);
        let state = app_state_with(
            vec![host.clone()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        put_running(
            &state,
            "i2",
            running_entry(
                "web",
                stale_local_url(),
                false,
                Some(remote_run(&host, &remote_url())),
            ),
        )
        .await;

        let server = PluginServer::new(
            COOKIE,
            vec![ok_response(serde_json::json!([
                {
                    "name": "@deepseek-ai/dsh-base",
                    "enabled": true,
                    "installed": true,
                    "removable": false,
                    "optional": false,
                    "rows": [{ "entryId": "llm" }, { "moduleName": "x" }]
                },
                {
                    "name": "ds-bot",
                    "enabled": true,
                    "installed": true,
                    "removable": true,
                    "optional": false,
                    "rows": [{ "entryId": "bot" }]
                }
            ]))],
        );
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();

        let view = list_plugins_via(&state, transport, "h2", "web", Some("i2"))
            .await
            .unwrap();

        assert_eq!(view.mode, PluginViewMode::Managed);
        assert_eq!(view.instance_id.as_deref(), Some("i2"));
        assert_eq!(
            view.bundles
                .iter()
                .map(|b| (b.name.as_str(), b.row_count))
                .collect::<Vec<_>>(),
            [("@deepseek-ai/dsh-base", 2), ("ds-bot", 1)],
            "每层带多少条目要如实报出来"
        );

        let forwards = forwarded(&hosts);
        assert_eq!(forwards.len(), 1, "一条实例地址一条转发");
        assert_eq!(forwards[0].remote_host, "192.0.2.7");
        assert_eq!(forwards[0].remote_port, 3080);
        assert_eq!(
            forwards[0].local_port, 3080,
            "本机端口与远端相同（没被占时）"
        );

        let minted = server.minted();
        assert_eq!(
            minted,
            vec![format!("http://127.0.0.1:3080/?token={TOKEN}")]
        );
        assert_eq!(
            server.last_sent().url,
            "http://127.0.0.1:3080/api/pluginManager/listBundles"
        );
        assert!(
            !server.last_sent().url.contains("192.0.2.7"),
            "请求不能打到那台机器自己的地址上：{}",
            server.last_sent().url
        );
    }

    #[tokio::test]
    async fn an_adopted_remote_instance_with_a_token_is_still_listed_over_the_forward() {
        let host = remote_host();
        let hosts = HostServer::new().answering(HostAnswer::Done);
        let state = app_state_with(
            vec![host.clone()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        put_running(
            &state,
            "i2",
            running_entry(
                "web",
                stale_local_url(),
                true,
                Some(remote_run(&host, &remote_url())),
            ),
        )
        .await;

        let server = PluginServer::new(
            COOKIE,
            vec![ok_response(serde_json::json!([
                {
                    "name": "ds-bot",
                    "enabled": true,
                    "installed": true,
                    "removable": true,
                    "optional": false,
                    "rows": [{ "entryId": "bot" }]
                }
            ]))],
        );
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();

        let view = list_plugins_via(&state, transport, "h2", "web", Some("i2"))
            .await
            .unwrap();

        assert_eq!(
            view.mode,
            PluginViewMode::Managed,
            "有 token 的接管实例照样能管"
        );
        assert_eq!(view.instance_id.as_deref(), Some("i2"));
        let forwards = forwarded(&hosts);
        assert_eq!(forwards.len(), 1, "一条实例地址一条转发");
        assert_eq!(
            server.minted(),
            vec![format!(
                "http://127.0.0.1:{}/?token={TOKEN}",
                forwards[0].local_port
            )],
            "列的是这个 Mac 够得着的地址，不是那台机器上的地址"
        );
    }

    #[tokio::test]
    async fn a_write_to_an_adopted_remote_instance_reaches_the_forwarded_address() {
        let host = remote_host();
        let hosts = HostServer::new().answering(HostAnswer::Done);
        let state = app_state_with(
            vec![host.clone()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        put_running(
            &state,
            "i2",
            running_entry(
                "web",
                stale_local_url(),
                true,
                Some(remote_run(&host, &remote_url())),
            ),
        )
        .await;

        let server = PluginServer::new(
            COOKIE,
            vec![ok_response(serde_json::json!({
                "changed": true,
                "application": "restart-required",
                "stage": "enable",
                "target": "ds-bot",
                "warnings": []
            }))],
        );
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();

        let client = write_client_via(&state, transport, "h2", "web", Some("i2"))
            .await
            .expect("有 token 的接管实例照样能写");
        let outcome = client.set_bundle_enabled("ds-bot", true).await.unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.application, "restart-required");
        let forwards = forwarded(&hosts);
        assert_eq!(forwards.len(), 1);
        let sent = server.last_sent();
        assert_eq!(
            sent.url,
            format!(
                "http://127.0.0.1:{}/api/pluginManager/setBundleEnabled",
                forwards[0].local_port
            )
        );
        assert_eq!(
            sent.envelope.payload.args,
            serde_json::json!({ "name": "ds-bot", "enabled": true })
        );
    }

    #[tokio::test]
    async fn a_local_instance_is_still_listed_over_its_own_address() {
        let home = scratch_home("local-managed");
        let state = app_state(
            vec![home_with_path("h1", &home)],
            vec![instance_of("i1", "h1")],
        );
        put_running(
            &state,
            "i1",
            running_entry("web", INSTANCE_URL.to_string(), false, None),
        )
        .await;
        let hosts = HostServer::new();
        let server = PluginServer::new(
            COOKIE,
            vec![ok_response(serde_json::json!([
                {
                    "name": "ds-bot",
                    "enabled": true,
                    "installed": true,
                    "removable": true,
                    "optional": false,
                    "rows": [{ "entryId": "bot" }]
                }
            ]))],
        );
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();
        let state = AppState {
            host_transport: hosts.clone(),
            ..state
        };

        let view = list_plugins_via(&state, transport, "h1", "web", Some("i1"))
            .await
            .unwrap();

        assert_eq!(view.mode, PluginViewMode::Managed);
        assert_eq!(server.minted(), vec![INSTANCE_URL]);
        assert_eq!(
            server.last_sent().url,
            "http://127.0.0.1:34567/api/pluginManager/listBundles"
        );
        assert!(hosts.calls().is_empty(), "本机实例不建转发");
    }

    #[tokio::test]
    async fn the_remote_projection_reads_that_machines_profile_in_one_call() {
        let manifest = profile_json(
            &["@deepseek-ai/dsh-base", "ds-bot"],
            &["@deepseek-ai/dsh-base"],
        );
        let hosts = HostServer::new().answering(HostAnswer::stdout(
            crate::remote::testing::profile_packages_stdout(
                &manifest,
                &[("@deepseek-ai/dsh-base", "0.2.1-alpha.2")],
            ),
        ));
        let state = app_state_with(
            vec![remote_host()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        let server = PluginServer::new(COOKIE, vec![]);
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();

        let view = list_plugins_via(&state, transport, "h2", "web", None)
            .await
            .unwrap();

        assert_eq!(view.mode, PluginViewMode::NotRunning);
        assert!(view.instance_id.is_none());
        let reason = view.reason.clone().expect("read-only reason");
        assert!(reason.contains("启动该实例"), "reason: {reason}");

        let names: Vec<&str> = view.bundles.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["@deepseek-ai/dsh-base", "ds-bot"]);
        let base = view
            .bundles
            .iter()
            .find(|b| b.name == "@deepseek-ai/dsh-base")
            .unwrap();
        assert!(base.enabled && base.installed);
        assert_eq!(
            base.version.as_deref(),
            Some("0.2.1-alpha.2"),
            "版本读自那台机器"
        );
        assert!(
            view.bundles.iter().all(|b| b.row_count == 0
                && !b.removable
                && b.availability.is_none()
                && b.error_code.is_none()),
            "文件投影故意比在线列表薄"
        );

        assert_eq!(hosts.calls().len(), 1, "一次 ssh 读回整个 Profile");
        assert!(
            hosts
                .last_script()
                .contains(&format!("{REMOTE_HOME}/profiles/web")),
            "读的是那台机器上的路径：{}",
            hosts.last_script()
        );
        assert!(server.minted().is_empty(), "没有实例可以问，就不该去连一个");
    }

    #[tokio::test]
    async fn an_unreachable_host_gives_a_readable_reason_and_asks_nothing() {
        let hosts = HostServer::new();
        let state = app_state_with(
            vec![remote_host()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        state.set_host_status(
            "host-lab",
            crate::config::HostStatus::unreachable(
                "ssh: connect to host zjq_aliyun port 22: Connection refused",
            ),
        );
        put_running(
            &state,
            "i2",
            running_entry(
                "web",
                stale_local_url(),
                false,
                Some(remote_run(&remote_host(), &remote_url())),
            ),
        )
        .await;
        let server = PluginServer::new(COOKIE, vec![]);
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();

        let view = list_plugins_via(&state, transport, "h2", "web", Some("i2"))
            .await
            .unwrap();

        assert_eq!(view.mode, PluginViewMode::Unreachable);
        let reason = view.reason.expect("read-only reason");
        assert!(reason.contains("服务器"), "要说清是哪台机器：{reason}");
        assert!(
            reason.contains("Connection refused"),
            "那台机器自己的话要带出来：{reason}"
        );
        assert!(
            reason.contains("重试"),
            "给出能离开这个状态的动作：{reason}"
        );
        assert!(view.bundles.is_empty(), "读不到就不假装读过");
        assert!(hosts.calls().is_empty(), "已经知道连不上，不再问一遍");
        assert!(server.minted().is_empty(), "也不去连实例");
    }

    #[tokio::test]
    async fn a_tunnel_that_will_not_come_up_says_so_instead_of_failing_silently() {
        let host = remote_host();
        let hosts = HostServer::new().answering(HostAnswer::fail(
            crate::ssh::HostErrorKind::Unreachable,
            "ssh: connect to host zjq_aliyun port 22: Connection refused",
        ));
        let state = app_state_with(
            vec![host.clone()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        put_running(
            &state,
            "i2",
            running_entry(
                "web",
                stale_local_url(),
                false,
                Some(remote_run(&host, &remote_url())),
            ),
        )
        .await;
        let server = PluginServer::new(COOKIE, vec![]);
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();

        let view = list_plugins_via(&state, transport, "h2", "web", Some("i2"))
            .await
            .unwrap();

        assert_eq!(view.mode, PluginViewMode::Unreachable);
        let reason = view.reason.expect("read-only reason");
        assert!(reason.contains("隧道"), "reason: {reason}");
        assert!(
            reason.contains("192.0.2.7"),
            "远端原始地址要念出来：{reason}"
        );
        assert!(
            reason.contains(&format!("ssh {TARGET}")),
            "给出可执行的命令：{reason}"
        );
        assert!(
            reason.contains("不是 DSH 拒绝"),
            "与「DSH 拒绝了这次操作」读起来不一样：{reason}"
        );
    }

    #[tokio::test]
    async fn a_remote_instance_without_a_token_is_read_only_and_names_the_adoption() {
        let manifest = profile_json(&["ds-bot"], &["ds-bot"]);
        let hosts = HostServer::new().answering(HostAnswer::stdout(
            crate::remote::testing::profile_packages_stdout(&manifest, &[("ds-bot", "0.9.4")]),
        ));
        let state = app_state_with(
            vec![remote_host()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        put_running(
            &state,
            "i2",
            running_entry(
                "web",
                "http://192.0.2.7:3080".to_string(),
                true,
                Some(remote_run(&remote_host(), "http://192.0.2.7:3080")),
            ),
        )
        .await;
        let server = PluginServer::new(COOKIE, vec![]);
        let transport: Arc<dyn crate::native::NativeTransport> = server.clone();

        let view = list_plugins_via(&state, transport, "h2", "web", Some("i2"))
            .await
            .unwrap();

        assert_eq!(view.mode, PluginViewMode::Adopted);
        let reason = view.reason.expect("read-only reason");
        assert!(reason.contains("接管"), "reason: {reason}");
        assert_eq!(
            view.bundles
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>(),
            ["ds-bot"]
        );
        assert_eq!(view.bundles[0].version.as_deref(), Some("0.9.4"));
        assert_eq!(hosts.calls().len(), 1, "投影仍是那台机器上一次调用");
        assert!(server.minted().is_empty(), "没有 token 就不该去连");
    }

    #[tokio::test]
    async fn a_write_to_an_unreachable_host_says_the_machine_is_the_problem() {
        let hosts = HostServer::new();
        let state = app_state_with(
            vec![remote_host()],
            vec![remote_home()],
            vec![remote_instance("i2")],
            hosts.clone(),
        );
        state.set_host_status(
            "host-lab",
            crate::config::HostStatus::unreachable("ssh 在 30 秒内没有返回"),
        );

        let err = match write_client(&state, "h2", "web", Some("i2")).await {
            Ok(_) => panic!("连不上的机器不能开始一次写入"),
            Err(e) => e,
        };

        assert!(err.contains("服务器"), "err: {err}");
        assert!(err.contains("重试"), "err: {err}");
        assert!(!err.contains("未在运行"), "不可达不是已停止：{err}");
    }
}
