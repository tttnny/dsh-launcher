// Profile plugins: per-instance/profile enable / disable / uninstall plumbing
// over the profile manifest (package.json) and cordis.patch.yml, driven
// through the instance's own `dsh plugin` CLI.

use crate::AppState;
use serde::{Deserialize, Serialize};
use tauri::State;

/// Public OAuth App client id used to boost unauthenticated GitHub API quota
/// from 60 to 5000 requests/hour (an anonymous client-id parameter, no
/// authorization or token storage required). App: "DSH Launcher".
const GITHUB_CLIENT_ID: &str = "Ov23li6vtlVd83282YL6";

/// Build a GitHub API URL with the anonymous client-id quota boost.
/// `pub(crate)` so `update.rs` (launcher self-update check) can reuse the
/// same quota-boosted endpoint instead of the rate-limited `releases.atom`.
pub(crate) fn github_api_url(path: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("https://api.github.com{path}{sep}client_id={GITHUB_CLIENT_ID}")
}

// ---------------------------------------------------------------------------
// Installed plugin (per instance/profile)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct InstalledPlugin {
    /// Package name / id (e.g. "@dsh-plugin/dsh-auxiliary").
    pub id: String,
    /// Installed version spec as recorded in the profile manifest.
    pub version: Option<String>,
    /// Whether the plugin is currently enabled (not disabled in cordis.patch.yml).
    pub enabled: bool,
    /// The cordis plugin id used in cordis.patch.yml (disables/insert rows).
    pub cordis_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPluginsEnabledInput {
    #[serde(alias = "home_id")]
    pub home_id: String,
    pub profile: String,
    #[serde(alias = "plugin_ids")]
    pub plugin_ids: Vec<String>,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

fn http_client() -> Result<reqwest::Client, String> {
    crate::proxy::apply(reqwest::Client::builder())
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("dsh-launcher")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

/// Fetch and parse a JSON document with a size cap.
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

// ---------------------------------------------------------------------------
// Profile manifest helpers (read/write package.json + cordis.patch.yml)
// ---------------------------------------------------------------------------

/// cordis id for a package: bundles register under their unscoped short name
/// (dsh-auxiliary) unless the package declares otherwise. We default to the
/// last path segment without the scope.
pub use crate::profile::cordis_id_of;

/// Read the profile package.json (dsh.profile.bundles + dependencies).
fn read_profile_manifest(dir: &std::path::Path) -> Result<serde_json::Value, String> {
    let path = dir.join("package.json");
    if !path.exists() {
        return Ok(serde_json::json!({
            "private": true,
            "dependencies": {},
            "dsh": { "profile": { "bundles": [] } },
        }));
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 package.json 失败: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("解析 package.json 失败: {e}"))
}

/// The packages a profile manifest declares, deduped and sorted: the
/// `dependencies` map (with their version specs) plus any names in
/// `dsh.profile.bundles` that no dependency already covered.
fn profile_packages(manifest: &serde_json::Value) -> Vec<(String, Option<String>)> {
    let mut packages: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    if let Some(deps) = manifest.get("dependencies").and_then(|d| d.as_object()) {
        for (name, spec) in deps {
            packages.insert(name.clone(), spec.as_str().map(str::to_string));
        }
    }
    if let Some(bundles) = manifest
        .pointer("/dsh/profile/bundles")
        .and_then(|b| b.as_array())
    {
        for b in bundles {
            if let Some(name) = b.as_str() {
                packages.entry(name.to_string()).or_insert(None);
            }
        }
    }
    let mut out: Vec<(String, Option<String>)> = packages.into_iter().collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------------------
// Commands: installed plugin listing (per instance + profile)
// ---------------------------------------------------------------------------

/// Lists plugins installed into a HOME's profile, excluding core
/// @deepseek-ai/* packages. Reads the profile manifest (dependencies +
/// bundles) and cordis.patch.yml (disabled rows).
#[tauri::command(rename_all = "snake_case")]
pub async fn list_installed_plugins(
    state: State<'_, AppState>,
    home_id: String,
    profile: String,
) -> Result<Vec<InstalledPlugin>, String> {
    let (home_path, _) = resolve_home_paths(&state, &home_id)?;
    let dir = crate::profile::profile_dir(&home_path, &profile)?;
    let manifest = read_profile_manifest(&dir)?;

    // Disabled set from cordis.patch.yml (`- id: <cordis-id>` + `disabled: true`).
    let disabled = crate::profile::read_disabled_ids(&dir);

    let out = profile_packages(&manifest)
        .into_iter()
        .filter(|(name, _)| !name.starts_with("@deepseek-ai/"))
        .map(|(id, version)| {
            let cordis_id = cordis_id_of(&id);
            let enabled = !disabled.contains(&cordis_id) && !disabled.contains(&id);
            InstalledPlugin {
                version,
                enabled,
                cordis_id: Some(cordis_id),
                id,
            }
        })
        .collect();
    Ok(out)
}

/// Resolve a HOME to (home_path, version_dir): plugin file edits need the
/// HOME path; `dsh plugin remove` runs through a CLI binary, so any
/// installed version serves (the newest installed one wins).
pub(crate) fn resolve_home_paths(
    state: &State<'_, AppState>,
    home_id: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let cfg = state.config.lock().unwrap();
    let home = cfg
        .homes
        .iter()
        .find(|h| h.id == home_id)
        .ok_or_else(|| "DSH_HOME 不存在".to_string())?;
    let version = cfg.versions.last().ok_or_else(|| "尚未安装任何 DSH 版本".to_string())?;
    Ok((home.path.clone(), version.dir.clone()))
}

// ---------------------------------------------------------------------------
// Commands: native plugin list (the instance's own plugin manager)
// ---------------------------------------------------------------------------

/// Lists the plugins of a profile of `(home_id, profile)`.
///
/// While an instance runs that profile, the list is the instance's own
/// **native plugin manager** answer (`POST /api/pluginManager/*`) — the running
/// DSH owns plugin state (ADR-0001), so the launcher does not read files to
/// build the list nor report entries the instance does not.
///
/// With **no instance running** the page is still usable, but read-only: the
/// list is read from the profile's own files (`package.json` + the patch
/// document), every control is disabled and the `reason` tells the user to
/// start the instance. That read path writes nothing — writing profile files is
/// exactly what ADR-0001 removed. An **adopted** instance (no usable token) and
/// an unreachable one take the same read-only path; their `reason` differs so
/// the two are distinguishable.
///
/// The instance is resolved from the selectors: the same DSH_HOME, running
/// the selected profile (see [`crate::process::running_for_profile`]). The
/// credential is the running-table URL's one-time token; the launcher never
/// reads `<DSH_HOME>/.credentials.yaml`.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_plugins(
    state: State<'_, AppState>,
    home_id: String,
    profile: String,
) -> Result<crate::native::PluginView, String> {
    list_plugins_for(state.inner(), &home_id, &profile).await
}

/// The body of [`list_plugins`], taking `&AppState` so tests can drive the
/// read-only states without a Tauri runtime.
async fn list_plugins_for(
    state: &AppState,
    home_id: &str,
    profile: &str,
) -> Result<crate::native::PluginView, String> {
    use crate::native::{NativeClient, PluginView, PluginViewMode};

    // The three read-only reasons are distinct on purpose: the UI shows them
    // verbatim, so each one has to say what happened and what to do.
    const NOT_RUNNING: &str = "实例未在运行：启动该实例后即可在此管理插件；下面是该 Profile 的插件（只读，从 Profile 文件读出）。";
    const ADOPTED: &str =
        "该实例由系统接管（DSH 自行重启），没有可用的启动 token，无法管理插件。请从启动器重启该实例。";

    let Some(entry) = crate::process::running_for_profile(state, home_id, profile).await? else {
        // Nothing is running this profile: read the profile's files (a
        // best-effort, read-only list) and tell the user how to manage.
        return offline_view(state, home_id, profile, PluginViewMode::NotRunning, NOT_RUNNING);
    };
    // An adopted instance has no recoverable token, so it cannot be managed.
    if entry.adopted {
        return offline_view(state, home_id, profile, PluginViewMode::Adopted, ADOPTED);
    }
    let Some(url) = entry.url else {
        return offline_view(state, home_id, profile, PluginViewMode::NotRunning, NOT_RUNNING);
    };

    // A refused session / unreachable instance is read-only rather than a
    // command error: the page still renders with the instance's reason.
    let client = match NativeClient::connect(&url).await {
        Ok(client) => client,
        Err(e) => return Ok(PluginView::readonly(PluginViewMode::Unreachable, e)),
    };
    let plugins = client.list_plugins().await?;
    let bundles = client.list_bundles().await?;
    Ok(PluginView::managed(plugins, bundles))
}

/// A read-only view listing a profile's plugins from its own files.
///
/// This is deliberately **best-effort and file-scoped**: it reads the profile
/// manifest (`dependencies` + `dsh.profile.bundles`) and the patch document's
/// disables, so it shows what the profile *declares*, not the instance's live
/// composition (no fiber phase, no availability, no error info — and the live
/// list additionally hides a few built-in bundles). A read failure degrades to
/// an empty list rather than erroring the page.
///
/// Unlike [`list_installed_plugins`], core `@deepseek-ai/*` packages are
/// **not** filtered out here. The blanket filter made this view lie about a
/// profile: it hid whether a core bundle is even declared, which is a fact a
/// read-only view has no business concealing. Their cordis id is usually the
/// package's unscoped short name (`@deepseek-ai/dsh-base` → `dsh-base`), the
/// same fallback the patch document uses to address them.
fn offline_view(
    state: &AppState,
    home_id: &str,
    profile: &str,
    mode: crate::native::PluginViewMode,
    reason: &str,
) -> Result<crate::native::PluginView, String> {
    Ok(crate::native::PluginView::readonly_files(
        mode,
        reason,
        offline_plugin_items(state, home_id, profile),
    ))
}

/// The profile's plugin entries read from its files, sorted by cordis id.
/// Unreadable files (or unknown HOME) yield an empty list — the read-only view
/// renders regardless.
fn offline_plugin_items(
    state: &AppState,
    home_id: &str,
    profile: &str,
) -> Vec<crate::native::PluginItem> {
    let Ok(home_path) = resolve_home_path(state, home_id) else {
        return Vec::new();
    };
    let Ok(dir) = crate::profile::profile_dir(&home_path, profile) else {
        return Vec::new();
    };
    let manifest = read_profile_manifest(&dir).unwrap_or_default();
    let disabled = crate::profile::read_disabled_ids(&dir);

    let mut items: Vec<crate::native::PluginItem> = profile_packages(&manifest)
        .into_iter()
        .map(|(name, _version)| {
            let cordis_id = cordis_id_of(&name);
            let enabled = !disabled.contains(&cordis_id) && !disabled.contains(&name);
            crate::native::PluginItem {
                entry_id: cordis_id.clone(),
                module_name: name,
                title: None,
                description: None,
                enabled,
                fiber_phase: None,
                patch_id: None,
                read_only_reason: None,
            }
        })
        .collect();
    items.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
    items
}

/// The HOME path of `home_id` (or an error), without resolving a CLI version —
/// the offline reader touches no CLI.
fn resolve_home_path(state: &AppState, home_id: &str) -> Result<std::path::PathBuf, String> {
    let cfg = state.config.lock().unwrap();
    cfg.homes
        .iter()
        .find(|h| h.id == home_id)
        .map(|h| h.path.clone())
        .ok_or_else(|| "DSH_HOME 不存在".to_string())
}

// ---------------------------------------------------------------------------
// Commands: bundle (patch layer) enable / disable (native)
// ---------------------------------------------------------------------------

/// Input for enabling/disabling a whole **Bundle（补丁层）**. The native
/// argument is the bundle `name` (not a plugin-entry id).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBundleEnabledInput {
    #[serde(alias = "home_id")]
    pub home_id: String,
    pub profile: String,
    pub name: String,
    pub enabled: bool,
}

/// Translates a native `ManagementError` code into a localized reason shown to
/// the user (one place for the whole app — design §4d). Refused operations are
/// not command errors; this explains *why* the instance would not change.
fn bundle_error_reason(code: &str) -> String {
    match code {
        "management-required" | "unaddressable" => {
            "该 Bundle 由 DSH 自身管理，不能在这里改动".to_string()
        }
        "not-removable" => "该 Bundle 随 DSH 发布，只能停用不能删除".to_string(),
        "unknown-plugin" => "实例里找不到这个 Bundle".to_string(),
        other => format!("实例拒绝了本次改动（{other}）"),
    }
}

/// Enables/disables one whole **Bundle（补丁层）** through the running
/// instance's native plugin manager (`setBundleEnabled`), so the launcher never
/// writes the profile manifest (`package.json`) or the patch document — the
/// running DSH owns plugin state (ADR-0001).
///
/// The instance is resolved the same way as [`list_plugins`]; without one (or
/// with an adopted one) the command is a plain `Err`. A **refusal** from the
/// instance (`changed:false` + `error.code`) is *not* an error: it is returned
/// as a [`ChangeOutcome`] carrying the translated `errorCode`, so the UI can
/// explain "this can't be changed" rather than "something broke" (design §4b).
///
/// Note the native semantics the UI surfaces: disabling removes the whole layer
/// (its entries and config disappear, dependencies remain); re-enabling
/// **appends** the bundle to the end of the profile's ordered list, changing
/// its priority.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_bundle_enabled(
    state: State<'_, AppState>,
    input: SetBundleEnabledInput,
) -> Result<ChangeOutcome, String> {
    set_bundle_enabled_for(
        state.inner(),
        &input.home_id,
        &input.profile,
        &input.name,
        input.enabled,
    )
    .await
}

/// The body of [`set_bundle_enabled`], taking `&AppState` so tests drive the
/// outcome branches without a Tauri runtime.
pub(crate) async fn set_bundle_enabled_for(
    state: &AppState,
    home_id: &str,
    profile: &str,
    name: &str,
    enabled: bool,
) -> Result<ChangeOutcome, String> {
    use crate::native::NativeClient;

    let Some(entry) = crate::process::running_for_profile(state, home_id, profile).await? else {
        return Err("实例未在运行，请先启动实例".to_string());
    };
    if entry.adopted {
        return Err("该实例由系统接管，无法管理插件".to_string());
    }
    let Some(url) = entry.url else {
        return Err("实例未在运行，请先启动实例".to_string());
    };
    // connect / transport / ok:false failures are real errors — the caller
    // re-syncs the list and shows the message.
    let client = NativeClient::connect(&url).await?;
    let result = client.set_bundle_enabled(name, enabled).await?;
    Ok(ChangeOutcome::from(result))
}

/// A write result handed to the frontend (design §4 frozen contract): the
/// native `ChangeResult`'s stable subset, `application` passed through as the
/// five-valued string, plus a localized `errorCode` reason for refusals.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeOutcome {
    pub changed: bool,
    /// applied | restart-required | overridden | failed | cancelled.
    pub application: String,
    /// enable | remove.
    pub stage: String,
    pub target: String,
    /// Localized reason for a refusal (`changed:false` + `error.code`); the UI
    /// shows it instead of a raw code.
    pub error_code: Option<String>,
    /// The instance's own diagnostic, when it reported one.
    pub diagnostic: Option<String>,
    pub warnings: Vec<String>,
}

impl From<crate::native::NativeChangeResult> for ChangeOutcome {
    fn from(r: crate::native::NativeChangeResult) -> Self {
        let (error_code, diagnostic) = match r.error {
            Some(e) => (Some(bundle_error_reason(&e.code)), e.diagnostic),
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

// ---------------------------------------------------------------------------
// Commands: enable / disable (cordis.patch.yml disabled rows)
// ---------------------------------------------------------------------------

/// Sets plugins enabled/disabled in a profile's cordis.patch.yml by adding or
/// removing `disabled: true` rows. Batch-capable via plugin_ids.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_plugins_enabled(
    state: State<'_, AppState>,
    input: SetPluginsEnabledInput,
) -> Result<(), String> {
    let (home_path, _) = resolve_home_paths(&state, &input.home_id)?;
    let dir = crate::profile::profile_dir(&home_path, &input.profile)?;
    let patch_path = dir.join("cordis.patch.yml");

    let mut raw = if patch_path.exists() {
        std::fs::read_to_string(&patch_path)
            .map_err(|e| format!("读取 cordis.patch.yml 失败: {e}"))?
    } else {
        String::new()
    };

    for package in &input.plugin_ids {
        let cordis_id = cordis_id_of(package);
        raw = crate::profile::set_disabled_row(&raw, &cordis_id, input.enabled);
    }

    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    std::fs::write(&patch_path, raw).map_err(|e| format!("写入 cordis.patch.yml 失败: {e}"))?;
    Ok(())
}

/// Input for uninstalling a plugin from a profile.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPluginInput {
    #[serde(alias = "home_id")]
    pub home_id: String,
    pub profile: String,
    #[serde(alias = "plugin_id")]
    pub plugin_id: String,
}

/// Uninstalls a plugin from a HOME's profile through
/// `dsh plugin --profile <name> remove <id>` (the CLI removes the dependency
/// and reconciles dsh.profile.bundles), then drops the plugin's
/// cordis.patch.yml rows (insert / disabled), which the CLI does not manage.
#[tauri::command(rename_all = "snake_case")]
pub async fn uninstall_plugin(
    state: State<'_, AppState>,
    input: UninstallPluginInput,
) -> Result<(), String> {
    let (home_path, version_dir) = resolve_home_paths(&state, &input.home_id)?;
    let dir = crate::profile::profile_dir(&home_path, &input.profile)?;
    if !dir.exists() {
        return Err(format!("Profile「{}」不存在", input.profile));
    }

    // `dsh plugin remove <id>` through an installed CLI: it removes the
    // dependency and reconciles dsh.profile.bundles (a name that is no
    // longer an installed bundle leaves the layer stack), so the manifest is
    // never edited by hand here. The frontend shows its own progress state.
    run_dsh_plugin(
        &state,
        &PluginCliTarget {
            version_dir: &version_dir,
            home_path: &home_path,
            profile: &input.profile,
        },
        &PluginCliOp {
            subcommand: "remove",
            spec: &input.plugin_id,
            loglevel: "warn",
        },
    )
    .await?;

    // 2. Drop the plugin's rows from cordis.patch.yml (insert rows mount the
    //    plugin; disabled rows gate it) — owned by the profile module.
    let patch_path = dir.join("cordis.patch.yml");
    if patch_path.exists() {
        let raw = std::fs::read_to_string(&patch_path)
            .map_err(|e| format!("读取 cordis.patch.yml 失败: {e}"))?;
        let cordis_id = cordis_id_of(&input.plugin_id);
        let cleaned = crate::profile::strip_cordis_rows(&raw, &cordis_id, &input.plugin_id);
        if cleaned != raw {
            std::fs::write(&patch_path, &cleaned)
                .map_err(|e| format!("写入 cordis.patch.yml 失败: {e}"))?;
        }
    }

    Ok(())
}

/// Which instance/profile a `dsh plugin` invocation targets.
struct PluginCliTarget<'a> {
    version_dir: &'a std::path::Path,
    home_path: &'a std::path::Path,
    profile: &'a str,
}

/// What the invocation does: a pnpm subcommand (`add` / `remove`), its
/// package spec, and the pnpm log level to forward.
struct PluginCliOp<'a> {
    subcommand: &'a str,
    spec: &'a str,
    loglevel: &'a str,
}

/// Runs one `dsh plugin --profile <name> <pnpm subcommand> <spec/id>` through
/// the instance's own CLI.
///
/// The launcher still prepares the two things the CLI does not: the
/// build-scripts opt-in (pnpm ≥10 `onlyBuiltDependencies` / pnpm 11
/// `allowBuilds`) and the profile `.npmrc` peer policy. When pnpm 11 blocks
/// build scripts it writes `set this to true or false` placeholders and fails
/// with ERR_PNPM_IGNORED_BUILDS; the placeholders are approved and the
/// invocation is retried once so native deps (node-pty, koffi, esbuild,
/// sharp…) actually build. The CLI prints the same advice for git-hosted
/// plugins, which this automates.
///
/// Short-lived and interactive: errors surface directly to the caller (the
/// frontend shows its own progress state and cancels by dropping the await).
async fn run_dsh_plugin(
    state: &State<'_, AppState>,
    target: &PluginCliTarget<'_>,
    op: &PluginCliOp<'_>,
) -> Result<(), String> {
    let (version_dir, home_path, profile) = (target.version_dir, target.home_path, target.profile);
    let (subcommand, spec, loglevel) = (op.subcommand, op.spec, op.loglevel);
    let dir = crate::profile::profile_dir(home_path, profile)?;
    // No instance is in scope here, so the flag comes from the profile's own
    // manifest: a `link:` dependency is exactly what needs it.
    let preserve_symlinks = crate::profile::declares_link_dependency(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    ensure_build_scripts_allowed(&dir)?;
    // Never let a plugin's peers pull a second copy of a core package in.
    ensure_profile_npmrc(&dir)?;
    // Record the store this invocation will use. This path always drives pnpm
    // with the launcher store (and relinks the profile onto it when it is
    // linked elsewhere), so the pin names that store — never the profile's
    // current one, which a relink a few lines below may replace.
    let store_dir = crate::toolchain::store_dir(&state.data_dir);
    ensure_profile_store_pin(&dir, &store_dir)?;

    let node = crate::runtime::node_for_spawn_checked(state).await?;
    let pnpm_prog = crate::toolchain::ensure_pnpm().await?;
    let what = format!("dsh plugin {subcommand}");

    // A node_modules tree linked from a *different* pnpm store makes pnpm
    // fail with ERR_PNPM_UNEXPECTED_STORE; relink proactively like the
    // installer path did.
    if let Some(linked) = linked_store_dir(&dir) {
        if !store_paths_match(&linked, &store_dir.to_string_lossy()) {
            crate::log_info!("node_modules 链接自其他 pnpm store（{linked}），重新链接后重试");
            relink_profile_store(state, target, &pnpm_prog).await?;
        }
    }

    for attempt in 1..=2 {
        let mut args: Vec<String> = vec![subcommand.to_string(), spec.to_string()];
        args.extend(forwarded_pnpm_flags(state, loglevel));
        let cmd =
            crate::launch::plugin_command(
                &node,
                version_dir,
                home_path,
                profile,
                &args,
                &pnpm_prog,
                preserve_symlinks,
            )?;
        match run_command(cmd, &what).await {
            Ok(()) => return Ok(()),
            Err(out) if attempt == 1 && mentions_ignored_builds(&out) => {
                crate::log_info!("pnpm 拦截了依赖构建脚本，批准 allowBuilds 后重试");
                ensure_build_scripts_allowed(&dir)?;
            }
            Err(out) if attempt == 1 && mentions_unexpected_store(&out) => {
                crate::log_info!("pnpm 报告 store 位置不一致，重新链接后重试");
                relink_profile_store(state, target, &pnpm_prog).await?;
            }
            Err(out) => return Err(format!("{what} 失败: {}", summarize_output(&out))),
        }
    }
    unreachable!("attempt loop covers both attempts")
}

/// Runs a piped child command, returning the FULL combined output on
/// failure: retryable-error detection (store mismatch, ignored builds)
/// matches markers like `ERR_PNPM_UNEXPECTED_STORE` that pnpm prints at the
/// START of its diagnostics, so callers must see the whole output. Display
/// sites compress it through [`summarize_output`].
async fn run_command(mut cmd: tokio::process::Command, what: &str) -> Result<(), String> {
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("{what} 启动失败: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if combined.trim().is_empty() {
        Err(format!("退出码 {}", out.status))
    } else {
        Err(combined)
    }
}

/// Compresses a command's full output into its last meaningful lines for
/// error display.
fn summarize_output(out: &str) -> String {
    let tail: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    tail.iter().rev().take(3).rev().cloned().collect::<Vec<&str>>().join(" | ")
}

fn mentions_unexpected_store(out: &str) -> bool {
    out.contains("ERR_PNPM_UNEXPECTED_STORE") || out.contains("Unexpected store location")
}

fn mentions_ignored_builds(out: &str) -> bool {
    out.contains("ERR_PNPM_IGNORED_BUILDS") || out.contains("Ignored build scripts")
}

/// Reads the store a profile's `node_modules` is currently linked from, via
/// the `storeDir` entry pnpm records in `node_modules/.modules.yaml`. Older
/// pnpm wrote YAML (`storeDir: /path`); pnpm 10+ writes JSON, so both
/// layouts are parsed.
fn linked_store_dir(profile_dir: &std::path::Path) -> Option<String> {
    let raw =
        std::fs::read_to_string(profile_dir.join("node_modules").join(".modules.yaml")).ok()?;
    for line in raw.lines() {
        if let Some(v) = line.trim().strip_prefix("storeDir:") {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) {
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

/// The store a profile's pnpm invocations should agree on: the one its
/// `node_modules` is already linked from, or the launcher's shared store for a
/// profile nothing has installed into yet.
///
/// Pinning to the *linked* store (rather than unconditionally to the
/// launcher's) keeps a profile that a user installed by hand against their
/// global store working: rewriting it onto the launcher store would only move
/// the mismatch, not resolve it. A linked path that is an ancestor/descendant
/// of the launcher store is normalized to the launcher's base dir, which is
/// the canonical spelling pnpm derives `<base>/v<major>` from.
pub(crate) fn profile_store_target(
    profile_dir: &std::path::Path,
    launcher_store: &std::path::Path,
) -> std::path::PathBuf {
    match linked_store_dir(profile_dir) {
        Some(linked) if !store_paths_match(&linked, &launcher_store.to_string_lossy()) => {
            std::path::PathBuf::from(linked)
        }
        _ => launcher_store.to_path_buf(),
    }
}

/// Quotes a path for a double-quoted YAML scalar. The launcher's data dir
/// contains a space (`.../Application Support/...`), which bare YAML would
/// truncate at, so quoting is mandatory rather than cosmetic.
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

/// Pins the profile's pnpm content store in `pnpm-workspace.yaml`.
///
/// The launcher drives every pnpm it spawns with an explicit `--store-dir`
/// (the shared store under the data dir), so profiles it installs into end up
/// linked from that store. Processes the launcher does **not** spawn have no
/// such flag: above all the DSH Web UI's own plugin manager, which runs bare
/// `pnpm` in the profile directory and therefore falls back to the user's
/// global store — which pnpm refuses with ERR_PNPM_UNEXPECTED_STORE for every
/// add/remove, the symptom this exists to remove. pnpm ≥10 reads its settings
/// from `pnpm-workspace.yaml` (not `.npmrc`), and an explicit `--store-dir`
/// still outranks the value written here, so recording the store in the
/// profile makes every pnpm that runs in it agree without changing what the
/// launcher's own invocations use.
pub(crate) fn ensure_profile_store_pin(
    dir: &std::path::Path,
    store_dir: &std::path::Path,
) -> Result<(), String> {
    let path = dir.join("pnpm-workspace.yaml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The same base document DSH's own `initProfile` writes, because
            // that writer never touches an existing file: creating a thinner
            // one here would permanently deny the profile its `nodeLinker:
            // hoisted` (the setting out-of-tree plugins depend on).
            "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n".to_string()
        }
        Err(e) => return Err(format!("读取 pnpm-workspace.yaml 失败: {e}")),
    };

    let pinned = format!(
        "storeDir: {}",
        yaml_double_quoted(&store_dir.to_string_lossy())
    );
    let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
    let mut found = false;
    let mut changed = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        // Only a top-level key counts: an indented `storeDir` belongs to some
        // nested mapping, and a commented one is not a setting.
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
        return Ok(());
    }

    std::fs::create_dir_all(dir).map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(&path, out).map_err(|e| format!("写入 pnpm-workspace.yaml 失败: {e}"))
}

/// Relinks a profile's `node_modules` onto the launcher's pinned store.
/// pnpm's own remedy for ERR_PNPM_UNEXPECTED_STORE is a reinstall, but a
/// plain `pnpm install` short-circuits with "Already up to date" when the
/// lockfile and node_modules already satisfy package.json — it never
/// re-links (verified against pnpm 11: `--force`, `--fix-lockfile`, even
/// deleting `.modules.yaml` all keep the stale store). The only reliable
/// relink is removing `node_modules` so pnpm must rebuild it from the
/// lockfile against the new store. `pnpm-lock.yaml` is preserved, so the
/// reinstall re-imports the same versions; it only costs the first
/// re-download into the launcher's store. Routing through
/// `dsh plugin install` keeps the CLI's bundle reconciliation in the loop.
async fn relink_profile_store(
    state: &State<'_, AppState>,
    target: &PluginCliTarget<'_>,
    pnpm_prog: &std::path::Path,
) -> Result<(), String> {
    let dir = crate::profile::profile_dir(target.home_path, target.profile)?;
    let nm = dir.join("node_modules");
    if nm.exists() {
        std::fs::remove_dir_all(&nm)
            .map_err(|e| format!("清理旧 node_modules 失败（{}）: {e}", nm.display()))?;
    }
    let mut args: Vec<String> = vec!["install".to_string()];
    args.extend(forwarded_pnpm_flags(state, "warn"));
    let preserve_symlinks = crate::profile::declares_link_dependency(&dir);
    let node = crate::runtime::node_for_spawn_checked(state).await?;
    let cmd = crate::launch::plugin_command(
        &node,
        target.version_dir,
        target.home_path,
        target.profile,
        &args,
        pnpm_prog,
        preserve_symlinks,
    )?;
    run_command(cmd, "dsh plugin install（重新链接 store）")
        .await
        .map_err(|e| {
            format!(
                "dsh plugin install（重新链接 store） 失败: {}",
                summarize_output(&e)
            )
        })
}


/// Common pnpm flags forwarded through `dsh plugin`. `--prefix` is deliberately
/// absent: the CLI already runs pnpm with cwd = the profile directory, and
/// passing a prefix would break that contract.
///
/// The flag *set* lives in [`crate::toolchain::pnpm_store_flags`] so this path
/// and the launcher's own installs can never drift apart; only the
/// The network-robustness settings are deliberately NOT forwarded as flags:
/// the `--config.fetch-*` spelling hangs pnpm 11 (see
/// [`crate::toolchain::pnpm_network_env`]). They ride along as env vars
/// instead, set on the CLI process by [`crate::launch::plugin_command`] so the
/// `pnpm` it spawns inherits them -- which also means `pnpm remove` no longer
/// needs special-casing, since env config is never rejected the way an
/// unknown flag is.
fn forwarded_pnpm_flags(state: &State<'_, AppState>, loglevel: &str) -> Vec<String> {
    let store_dir = crate::toolchain::store_dir(&state.data_dir);
    crate::toolchain::pnpm_store_flags(&store_dir, loglevel)
}

/// Pins `auto-install-peers=false` in a profile's `.npmrc`.
///
/// A DSH profile must resolve nothing from the `@deepseek-ai` core scope —
/// core comes from the CLI's own dependency tree. With auto-install-peers on
/// (a common global pnpm setting), installing a plugin whose peers include a
/// core package drops a second copy of that core package into the profile,
/// which is the duplicated-Symbol failure the doctor check reports. Writing
/// the setting per profile makes the install independent of the user's global
/// pnpm configuration.
pub(crate) fn ensure_profile_npmrc(dir: &std::path::Path) -> Result<(), String> {
    const KEY: &str = "auto-install-peers";
    let path = dir.join(".npmrc");
    let raw = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| format!("读取 .npmrc 失败: {e}"))?
    } else {
        String::new()
    };

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
        // Keep a short rationale in the file: it is user-visible state.
        if !lines.is_empty() && !lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            lines.push(String::new());
        }
        lines.push("# DSH: core packages come from the CLI dependency tree;".to_string());
        lines.push("# a profile must never resolve its own copy.".to_string());
        lines.push(format!("{KEY}=false"));
        changed = true;
    }
    if changed {
        let mut out = lines.join("\n");
        out.push('\n');
        std::fs::write(&path, out).map_err(|e| format!("写入 .npmrc 失败: {e}"))?;
    }
    Ok(())
}

/// Make sure a profile's pnpm-workspace.yaml opts into dependency build
/// scripts on both pnpm 10 (onlyBuiltDependencies) and pnpm 11 (allowBuilds).
///
/// pnpm 11 writes `allowBuilds: <name>: set this to true or false` for every
/// dependency whose build script it ignored, then fails the install with
/// ERR_PNPM_IGNORED_BUILDS. This converts those placeholders to `true` and
/// keeps the old field around for pnpm ≤10, so a subsequent install actually
/// runs the native build scripts (node-pty, koffi, esbuild, sharp, …).
pub(crate) fn ensure_build_scripts_allowed(dir: &std::path::Path) -> Result<(), String> {
    let ws_manifest = dir.join("pnpm-workspace.yaml");
    let raw = if ws_manifest.exists() {
        std::fs::read_to_string(&ws_manifest)
            .map_err(|e| format!("读取 pnpm-workspace.yaml 失败: {e}"))?
    } else {
        String::new()
    };

    // Base document: `packages` is required for pnpm to treat the dir as a
    // workspace (needed for allowBuilds to be read from this file).
    let mut lines: Vec<String> = if raw.trim().is_empty() {
        vec!["packages:".to_string(), "  - .".to_string()]
    } else {
        raw.lines().map(|l| l.to_string()).collect()
    };

    // 1. Convert pnpm-11 placeholder values ("set this to true or false") to
    //    real booleans so the next install builds those packages.
    let mut changed = false;
    for line in lines.iter_mut() {
        if line.contains("set this to true or false") {
            *line = line.replace("set this to true or false", "true");
            changed = true;
        }
    }

    // 2. Ensure the legacy `onlyBuiltDependencies: ['*']` block exists
    //    (pnpm ≤10 reads only this field).
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

    if changed {
        let out = lines.join("\n");
        if !out.ends_with('\n') {
            lines.push(String::new());
        }
        std::fs::write(&ws_manifest, lines.join("\n"))
            .map_err(|e| format!("写入 pnpm-workspace.yaml 失败: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cordis_id_of_strips_scope_and_org() {
        assert_eq!(cordis_id_of("@dsh-plugin/dsh-auxiliary"), "dsh-auxiliary");
        assert_eq!(cordis_id_of("@dsh-external/dsh-sidechain"), "dsh-sidechain");
        assert_eq!(cordis_id_of("dsh-better-sidebar"), "dsh-better-sidebar");
        assert_eq!(cordis_id_of("@canglongcl/dsh-web-review"), "dsh-web-review");
    }

    #[test]
    fn store_paths_match_handles_versioned_subdir_and_slashes() {
        let base = "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store";
        // `.modules.yaml` records the versioned subdir pnpm derived from the
        // pinned base.
        assert!(store_paths_match(&format!("{base}/v11"), base));
        // A trailing separator is equivalent.
        assert!(store_paths_match(
            "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store/v11/",
            base
        ));
        // A genuinely different store (the user's global one) mismatches.
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
        // Missing file → None (fresh profile, nothing to relink).
        assert_eq!(linked_store_dir(&dir), None);
    }

    #[test]
    fn linked_store_dir_reads_json_modules_yaml() {
        // pnpm 10+ writes `.modules.yaml` as JSON; the line-based YAML scan
        // misses `"storeDir": "..."`, which used to silently skip relinking.
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
    fn summarize_output_keeps_last_three_meaningful_lines() {
        let out = "ERR_PNPM_UNEXPECTED_STORE: some long diagnostic\nmiddle line\n\nlast line\n";
        assert_eq!(
            summarize_output(out),
            "ERR_PNPM_UNEXPECTED_STORE: some long diagnostic | middle line | last line"
        );
        // Empty output stays empty.
        assert_eq!(summarize_output("\n  \n"), "");
    }

    /// The regression this fix exists for: a profile installed through the
    /// launcher is linked from the launcher's store, while the Web UI's own
    /// plugin manager runs bare `pnpm` (no `--store-dir`) and would otherwise
    /// fall back to the global store and die with ERR_PNPM_UNEXPECTED_STORE.
    /// The pin has to land in `pnpm-workspace.yaml`, the file pnpm ≥10 reads.
    #[test]
    fn ensure_profile_store_pin_writes_workspace_setting() {
        let dir = std::env::temp_dir().join(format!("dsh-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ws = dir.join("pnpm-workspace.yaml");
        let store = "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store";

        // Existing workspace document: the key is appended, existing settings
        // (which the CLI's own template owns) survive.
        std::fs::write(
            &ws,
            "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n",
        )
        .unwrap();
        ensure_profile_store_pin(&dir, std::path::Path::new(store)).unwrap();
        let pinned = std::fs::read_to_string(&ws).unwrap();
        // The path contains a space, so it must be quoted or YAML truncates it.
        assert!(
            pinned.contains(&format!("storeDir: \"{store}\"")),
            "pinned: {pinned}"
        );
        assert!(pinned.contains("nodeLinker: hoisted"), "pinned: {pinned}");
        assert!(
            pinned.contains("autoInstallPeers: false"),
            "pinned: {pinned}"
        );

        // Idempotent: a second run leaves the bytes alone.
        ensure_profile_store_pin(&dir, std::path::Path::new(store)).unwrap();
        assert_eq!(std::fs::read_to_string(&ws).unwrap(), pinned);

        // A moved store is replaced in place, never duplicated.
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
        // A fresh profile must end up with the same base document DSH's own
        // `initProfile` writes: it never touches an existing file, so a
        // thinner file here would permanently deny the profile its
        // `nodeLinker: hoisted`.
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
        // An indented `storeDir` belongs to a nested mapping and a commented
        // one is not a setting; rewriting either would corrupt the document.
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

    /// The target chooses which store to pin: a profile already linked from
    /// the launcher store keeps that (canonical base-dir) spelling, while one
    /// a user installed by hand against their global store keeps its own — the
    /// pin must not drag a working profile onto a different store.
    #[test]
    fn profile_store_target_follows_the_linked_store() {
        let dir = std::env::temp_dir().join(format!("dsh-store-test-{}", uuid::Uuid::new_v4()));
        let nm = dir.join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        let launcher = "/Users/x/Library/Application Support/in.dsh-plug.dsh-launcher/.pnpm-store";

        // No node_modules record: nothing to preserve, pin the launcher store.
        assert_eq!(
            profile_store_target(&dir, std::path::Path::new(launcher)),
            std::path::PathBuf::from(launcher)
        );

        // Linked from the launcher store via its versioned subdir: normalize
        // to the base dir pnpm derives `<base>/v11` from.
        std::fs::write(
            nm.join(".modules.yaml"),
            format!("{{\"storeDir\": \"{launcher}/v11\"}}\n"),
        )
        .unwrap();
        assert_eq!(
            profile_store_target(&dir, std::path::Path::new(launcher)),
            std::path::PathBuf::from(launcher)
        );

        // Linked from the user's global store: that store is the target.
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

    
    
    
    
    
    // Live network smoke tests (skipped by default; run with
    // `cargo test plugins::tests::live_ -- --ignored`).
    #[test]
    fn ensure_build_scripts_allowed_converts_placeholders_and_adds_sections() {
        let dir = std::env::temp_dir().join(format!("dsh-plugins-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Fresh profile: no workspace file yet -> packages + both sections.
        ensure_build_scripts_allowed(&dir).unwrap();
        let fresh = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(fresh.contains("packages:"), "fresh: {fresh}");
        assert!(fresh.contains("onlyBuiltDependencies"), "fresh: {fresh}");
        assert!(fresh.contains("allowBuilds:"), "fresh: {fresh}");

        // pnpm 11 left a placeholder behind after ERR_PNPM_IGNORED_BUILDS.
        std::fs::write(
            dir.join("pnpm-workspace.yaml"),
            "packages:\n  - .\nallowBuilds:\n  node-pty: set this to true or false\n",
        )
        .unwrap();
        ensure_build_scripts_allowed(&dir).unwrap();
        let fixed = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert!(fixed.contains("node-pty: true"), "fixed: {fixed}");
        assert!(
            !fixed.contains("set this to true or false"),
            "fixed: {fixed}"
        );
        // Legacy section added without clobbering existing content.
        assert!(fixed.contains("onlyBuiltDependencies"), "fixed: {fixed}");

        // Idempotent: second run leaves the file unchanged.
        let before = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        ensure_build_scripts_allowed(&dir).unwrap();
        let after = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert_eq!(before, after, "must be idempotent");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_profile_npmrc_pins_auto_install_peers_false() {
        let dir = std::env::temp_dir().join(format!("dsh-npmrc-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let npmrc = dir.join(".npmrc");

        // Fresh profile: the key is written.
        ensure_profile_npmrc(&dir).unwrap();
        let fresh = std::fs::read_to_string(&npmrc).unwrap();
        assert!(fresh.contains("auto-install-peers=false"), "fresh: {fresh}");

        // Idempotent.
        ensure_profile_npmrc(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&npmrc).unwrap(), fresh);

        // An opposite existing value is normalized, other keys are preserved.
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

        // A commented-out key is not treated as set.
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
        AppState {
            config_path: std::path::PathBuf::new(),
            data_dir: std::path::PathBuf::new(),
            config: std::sync::Mutex::new(crate::config::Config {
                homes,
                versions: Vec::new(),
                instances,
                settings: Default::default(),
            }),
            running: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            tasks: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// A scratch DSH_HOME under the repo's gitignored `tmp/` (never the system
    /// temp dir). The unique name keeps parallel test processes apart.
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
        }
    }

    fn instance_of(id: &str, home_id: &str) -> crate::config::DshInstance {
        crate::config::DshInstance {
            id: id.to_string(),
            name: id.to_string(),
            version_id: "v1".to_string(),
            home_id: home_id.to_string(),
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
            preserve_symlinks: false,
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

    /// Writes a profile dir under a scratch HOME and returns (HOME, profile dir).
    fn seeded_home(files: &[(&str, &str)], profile: &str) -> std::path::PathBuf {
        let home = scratch_home("home");
        let dir = crate::profile::profile_dir(&home, profile).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            std::fs::write(dir.join(name), content).unwrap();
        }
        home
    }

    /// No instance is running this profile: the page is read-only in
    /// `not-running` mode, but it still lists what the profile has — read from
    /// its files (dependencies + bundles), with no `@deepseek-ai/*` filter, and
    /// the patch document's native **flow-style** disables understood.
    #[tokio::test]
    async fn list_plugins_for_profile_files_when_not_running() {
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
        let state = app_state(vec![home_with_path("h1", &home)], vec![instance_of("i1", "h1")]);

        let view = list_plugins_for(&state, "h1", "web").await.unwrap();

        assert_eq!(view.mode, crate::native::PluginViewMode::NotRunning);
        // The reason is distinct and tells the user what to do.
        let reason = view.reason.expect("read-only reason");
        assert!(reason.contains("启动该实例"), "reason: {reason}");

        // Both the plugin package and the core package show (no blanket
        // @deepseek-ai/* filter); deduped across dependencies + bundles, and
        // sorted by the entry id the table shows.
        let names: Vec<&str> = view.plugins.iter().map(|p| p.module_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["@dsh-plugin/dsh-auxiliary", "@deepseek-ai/dsh-base"],
            "names: {names:?}"
        );

        let aux = view
            .plugins
            .iter()
            .find(|p| p.module_name == "@dsh-plugin/dsh-auxiliary")
            .unwrap();
        // The flow-style disabled row was seen: the entry reads as off.
        assert!(!aux.enabled, "dsh-auxiliary must read disabled: {aux:?}");
        let base = view
            .plugins
            .iter()
            .find(|p| p.module_name == "@deepseek-ai/dsh-base")
            .unwrap();
        assert!(base.enabled, "dsh-base must read enabled: {base:?}");
    }

    /// The read path writes nothing: every profile file is byte-identical and
    /// untouched (mtime unchanged) across a read.
    #[tokio::test]
    async fn list_plugins_read_path_leaves_files_untouched() {
        let home = seeded_home(
            &[
                (
                    "package.json",
                    &profile_json(&["@dsh-plugin/dsh-auxiliary"], &["@deepseek-ai/dsh-base"]),
                ),
                (
                    "cordis.patch.yml",
                    "[ { id: dsh-auxiliary, disabled: true } ]\n",
                ),
            ],
            "web",
        );
        let dir = crate::profile::profile_dir(&home, "web").unwrap();
        let state = app_state(vec![home_with_path("h1", &home)], vec![instance_of("i1", "h1")]);

        let before: Vec<(std::path::PathBuf, Vec<u8>, std::time::SystemTime)> =
            ["package.json", "cordis.patch.yml"]
                .iter()
                .map(|n| {
                    let p = dir.join(n);
                    let bytes = std::fs::read(&p).unwrap();
                    let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
                    (p, bytes, mtime)
                })
                .collect();

        let _ = list_plugins_for(&state, "h1", "web").await.unwrap();
        // The directory listing is unchanged too (nothing created/removed).
        let listing_before = dir.read_dir().unwrap().count();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = list_plugins_for(&state, "h1", "web").await.unwrap();

        for (path, bytes, mtime) in before {
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "bytes changed: {path:?}");
            assert_eq!(
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                mtime,
                "mtime changed: {path:?}"
            );
        }
        assert_eq!(dir.read_dir().unwrap().count(), listing_before);
    }

    /// An adopted instance carries a bare URL and no token, so the view is
    /// `adopted` — distinct from `not-running` — and its reason names the
    /// adoption, not "start the instance".
    #[tokio::test]
    async fn list_plugins_marks_adopted_instances_readonly() {
        let home = scratch_home("adopted");
        let state = app_state(vec![home_with_path("h1", &home)], vec![instance_of("i1", "h1")]);
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
                },
            );
        }
        let view = list_plugins_for(&state, "h1", "web").await.unwrap();
        assert_eq!(view.mode, crate::native::PluginViewMode::Adopted);
        assert!(view.plugins.is_empty());
        let adopted_reason = view.reason.expect("adopted reason");
        assert!(
            adopted_reason.contains("接管"),
            "adopted reason must name the adoption: {adopted_reason}"
        );

        // And it is distinguishable from the not-running message.
        let not_running = crate::native::PluginView::readonly(
            crate::native::PluginViewMode::NotRunning,
            "实例未运行",
        );
        assert_ne!(Some(adopted_reason), not_running.reason);
    }

    /// Managing a bundle requires the profile's instance to be running: with
    /// none, the command is a plain `Err` (not a refusal outcome) so the UI
    /// re-syncs and tells the user to start the instance.
    #[tokio::test]
    async fn set_bundle_enabled_refuses_without_a_running_instance() {
        let state = app_state(
            vec![crate::config::DshHome {
                id: "h1".to_string(),
                name: "h1".to_string(),
                path: std::path::PathBuf::from("/tmp/home"),
            }],
            vec![crate::config::DshInstance {
                id: "i1".to_string(),
                name: "i1".to_string(),
                version_id: "v1".to_string(),
                home_id: "h1".to_string(),
                env_overrides: Default::default(),
                default_profile: None,
                last_profile: None,
                icon: None,
                port: None,
                preserve_symlinks: false,
            }],
        );
        let err = set_bundle_enabled_for(&state, "h1", "web", "@deepseek-ai/dsh-llm", false)
            .await
            .unwrap_err();
        assert!(err.contains("未在运行"), "err: {err}");
    }

    /// An adopted instance has no recoverable token, so its bundles cannot be
    /// managed through the native API — a distinct message from "not running".
    #[tokio::test]
    async fn set_bundle_enabled_refuses_for_an_adopted_instance() {
        let state = app_state(
            vec![crate::config::DshHome {
                id: "h1".to_string(),
                name: "h1".to_string(),
                path: std::path::PathBuf::from("/tmp/home"),
            }],
            vec![crate::config::DshInstance {
                id: "i1".to_string(),
                name: "i1".to_string(),
                version_id: "v1".to_string(),
                home_id: "h1".to_string(),
                env_overrides: Default::default(),
                default_profile: None,
                last_profile: None,
                icon: None,
                port: None,
                preserve_symlinks: false,
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
                },
            );
        }
        let err = set_bundle_enabled_for(&state, "h1", "web", "@deepseek-ai/dsh-llm", false)
            .await
            .unwrap_err();
        assert!(err.contains("接管"), "err: {err}");
    }
}
