// Profile plugins: the instance-native plugin commands (list / enable /
// disable / remove) over the running instance's own `/api/pluginManager/*`
// service, plus the profile-file preparation the launcher's own pnpm needs
// (build scripts, peer policy, pnpm store pin). The launcher never writes
// plugin state — the running DSH owns it (ADR-0001).

use crate::AppState;
use crate::native::NativeChangeResult;
use serde::Deserialize;
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
// Profile manifest helpers (read-only package.json + cordis.patch.yml)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Commands: native plugin list (the instance's own plugin manager)
// ---------------------------------------------------------------------------

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
/// The instance is resolved from the selectors: the same DSH_HOME, running the
/// selected profile (see [`crate::process::running_for_profile`]). The
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

    // The read-only reasons are distinct on purpose: the UI shows them
    // verbatim, so each one has to say what happened and what to do.
    const NOT_RUNNING: &str =
        "实例未在运行：启动该实例后即可在这里管理它的补丁层。";
    const ADOPTED: &str =
        "该实例由系统接管（DSH 自行重启），没有可用的启动 token，无法管理补丁层。请从启动器重启该实例。";

    let Some(entry) = crate::process::running_for_profile(state, home_id, profile).await? else {
        return Ok(PluginView::readonly(PluginViewMode::NotRunning, NOT_RUNNING));
    };
    // What decides is the token, not how we came to own the entry: an adopted
    // process whose launch URL we remembered is as manageable as a child of
    // ours, and the bare URL of one we cannot authenticate is not.
    let Some(url) = crate::process::manageable_url(&entry) else {
        let mode = if entry.adopted {
            PluginViewMode::Adopted
        } else {
            PluginViewMode::NotRunning
        };
        let reason = if entry.adopted { ADOPTED } else { NOT_RUNNING };
        return Ok(PluginView::readonly(mode, reason));
    };
    let url = url.to_string();

    // A refused session / unreachable instance is read-only rather than a
    // command error: the page still renders with the instance's reason.
    let client = match NativeClient::connect(&url).await {
        Ok(client) => client,
        Err(e) => return Ok(PluginView::readonly(PluginViewMode::Unreachable, e)),
    };
    let bundles = client.list_bundles().await?;
    Ok(PluginView::managed(bundles, &ui_locale(state)))
}

/// The launcher's own UI language. The service localizes its metadata
/// (`meta.title` may be a `{en, zh}` map), so the projection needs to know
/// which language this app is showing.
pub(crate) fn ui_locale(state: &AppState) -> String {
    state.config.lock().unwrap().settings.locale.clone()
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

/// Enables/disables one whole **Bundle（补丁层）** through the running
/// instance's native plugin manager (`setBundleEnabled`), so the launcher never
/// writes the profile manifest (`package.json`) or the patch document — the
/// running DSH owns plugin state (ADR-0001).
///
/// The instance is resolved the same way as [`list_plugins`]; without one (or
/// with an adopted one) the command is a plain `Err`. A **refusal** from the
/// instance (`changed:false` + `error.code`) is *not* an error: it is returned
/// as a [`ChangeOutcome`] carrying the raw `errorCode` and the localized reason
/// in `diagnostic`, so the UI can explain "this can't be changed" rather than
/// "something broke" (design §4b).
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
/// outcome branches without a Tauri runtime. Shares the write path
/// ([`write_client`]) with T2/T4, and funnels the result through
/// [`localize_refusal`] so every write command returns the same shape.
pub(crate) async fn set_bundle_enabled_for(
    state: &AppState,
    home_id: &str,
    profile: &str,
    name: &str,
    enabled: bool,
) -> Result<ChangeOutcome, String> {
    let client = write_client(state, home_id, profile).await?;
    let result = client.set_bundle_enabled(name, enabled).await?;
    Ok(localize_refusal(ChangeOutcome::from_native(result, &ui_locale(state))))
}

/// A write result handed to the frontend (design §4 frozen contract): the
/// native `ChangeResult`'s stable subset, `application` passed through as the
/// five-valued string, plus the native `errorCode` and the localized
/// `diagnostic` for refusals.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeOutcome {
    pub changed: bool,
    /// applied | restart-required | overridden | failed | cancelled.
    pub application: String,
    /// enable | remove.
    pub stage: String,
    pub target: String,
    /// The **raw** native `error.code` for a refusal (`changed:false`); logic,
    /// not display — the localized reason rides in `diagnostic`.
    pub error_code: Option<String>,
    /// The localized reason a refusal gives, folded in by [`localize_refusal`];
    /// the instance's own diagnostic otherwise. The UI shows it verbatim.
    pub diagnostic: Option<String>,
    pub warnings: Vec<String>,
}

impl ChangeOutcome {
    /// Projects one native write result, resolving the instance's localized
    /// diagnostic with the launcher's own UI locale.
    pub(crate) fn from_native(r: NativeChangeResult, locale: &str) -> Self {
        // The raw code is kept as-is; the localized reason is added later by
        // `localize_refusal`, the single place every write command funnels
        // through (design §4d).
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

// ---------------------------------------------------------------------------
// Commands: bundle (patch layer) removal (native)
// ---------------------------------------------------------------------------

/// Input for removing a whole **Bundle（补丁层）**. The native argument is the
/// bundle `name` (not a plugin-entry id).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveBundleInput {
    #[serde(alias = "home_id")]
    pub home_id: String,
    pub profile: String,
    pub name: String,
}

/// Removes one whole **Bundle（补丁层）** through the running instance's native
/// plugin manager (`removeBundle`) — the same path the Web UI uses, so a plugin
/// the Web UI can remove the launcher can too, and the launcher still never
/// writes the profile manifest (`package.json`) or the patch document
/// (ADR-0001).
///
/// The instance is resolved the same way as [`list_plugins`]; without one (or
/// with an adopted one) the command is a plain `Err`. A bundle that ships with
/// DSH is **refused** by the instance (`changed:false` +
/// `error.code:"not-removable"`): that is returned as a [`ChangeOutcome`] with
/// the localized reason folded into `diagnostic`, so the page explains "this
/// can't be removed" rather than "something broke" (design §4b). The UI
/// disables the entry for `removable:false` bundles in the first place; this
/// path is the truthful answer if a refusal still arrives.
#[tauri::command(rename_all = "snake_case")]
pub async fn remove_bundle(
    state: State<'_, AppState>,
    input: RemoveBundleInput,
) -> Result<ChangeOutcome, String> {
    remove_bundle_for(state.inner(), &input).await
}

/// The body of [`remove_bundle`], taking `&AppState` so tests drive the
/// resolve branches without a Tauri runtime. Shares the write path
/// ([`write_client`]) with T2/T3.
pub(crate) async fn remove_bundle_for(
    state: &AppState,
    input: &RemoveBundleInput,
) -> Result<ChangeOutcome, String> {
    let client = write_client(state, &input.home_id, &input.profile).await?;
    let result = client.remove_bundle(&input.name).await?;
    Ok(localize_refusal(ChangeOutcome::from_native(result, &ui_locale(state))))
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

/// Resolves the instance that can manage `(home_id, profile)`'s plugins, as a
/// connected [`NativeClient`](crate::native::NativeClient). The refusal messages
/// are the ones the page shows for the "cannot manage now" states.
pub(crate) async fn write_client(
    state: &AppState,
    home_id: &str,
    profile: &str,
) -> Result<crate::native::NativeClient, String> {
    let Some(entry) = crate::process::running_for_profile(state, home_id, profile).await? else {
        return Err("实例未在运行，请先启动实例".to_string());
    };
    // Same rule as the read path: a token is what makes the instance
    // addressable, however the entry came to exist.
    let Some(url) = crate::process::manageable_url(&entry) else {
        return Err(if entry.adopted {
            "该实例由系统接管，无法管理插件".to_string()
        } else {
            "实例未在运行，请先启动实例".to_string()
        });
    };
    crate::native::NativeClient::connect(url).await
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

    /// The entry a bundle ships takes that bundle's version, so the profile's
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
    /// Without a running instance the page is read-only: the reason says what
    /// to do, and there is nothing to manage until one is up — the bundle list
    /// is the *instance's* answer, so the launcher does not guess it from files.
    #[tokio::test]
    async fn list_plugins_is_read_only_and_empty_when_not_running() {
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
        assert!(view.bundles.is_empty(), "no instance, no bundle list");
        // The reason is distinct and tells the user what to do.
        let reason = view.reason.expect("read-only reason");
        assert!(reason.contains("启动该实例"), "reason: {reason}");
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
        assert!(view.bundles.is_empty());
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

    /// Adoption is not what blocks management: when the launcher remembered
    /// the adopted process's launch URL, the write path goes on to connect
    /// like any other. Only the connection itself may then fail.
    #[tokio::test]
    async fn an_adopted_instance_with_a_token_is_still_addressed() {
        let home = scratch_home("adopted-token");
        let state = app_state(vec![home_with_path("h1", &home)], vec![instance_of("i1", "h1")]);
        {
            let mut running = state.running.lock().await;
            running.insert(
                "i1".to_string(),
                crate::process::RunningInstance {
                    kill: std::sync::Arc::new(tokio::sync::Notify::new()),
                    profile: "web".to_string(),
                    // Port 1 refuses immediately: the failure has to come from
                    // the connection, not from a refusal to try.
                    url: Some("http://127.0.0.1:1/?token=remembered".to_string()),
                    adopted: Some(crate::process::Adopted {
                        pid: 1,
                        port: 1,
                        host: "127.0.0.1".to_string(),
                    }),
                },
            );
        }
        let err = match write_client(&state, "h1", "web").await {
            Ok(_) => panic!("no instance is reachable in this test"),
            Err(e) => e,
        };
        assert!(
            !err.contains("接管"),
            "an adopted instance we hold a token for must be attempted, got: {err}"
        );
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

    fn one_home() -> Vec<crate::config::DshHome> {
        vec![crate::config::DshHome {
            id: "h1".to_string(),
            name: "h1".to_string(),
            path: std::path::PathBuf::from("/tmp/home"),
        }]
    }

    fn one_instance() -> Vec<crate::config::DshInstance> {
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
        }]
    }

    /// The refusal code becomes the reason the page shows verbatim; this is the
    /// one place codes are translated (design §4d), never the UI.
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
        // An unlisted code is surfaced, never swallowed.
        assert_eq!(refusal_reason("operation-error", None), "operation-error");
        // A missing code still reads as a sentence, not an empty string.
        assert_eq!(refusal_reason("", None), "实例未说明原因");
        // The instance's diagnostic is attached when present.
        assert_eq!(
            refusal_reason("management-required", Some("managed by DSH")),
            "该条目由 DSH 自身管理，不能在这里改动（managed by DSH）"
        );
    }

    /// The frontend shape: the native error is lifted to `errorCode`/`diagnostic`
    /// and serialization is camelCase, matching `src/api/types.ts::ChangeOutcome`.
    /// The projection lives next to [`ChangeOutcome`] here, so its test does too.
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
        // `errorCode` carries the **raw native code** (logic); the instance's
        // own diagnostic rides alongside it untouched. The localized reason is
        // folded in later, by `localize_refusal`, for every write command.
        assert_eq!(outcome.error_code.as_deref(), Some("unaddressable"));
        assert_eq!(outcome.diagnostic.as_deref(), Some("root group"));

        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["errorCode"], "unaddressable");
        assert_eq!(json["application"], "failed");
        assert_eq!(json["stage"], "enable");
        assert_eq!(json["target"], "include:llm");
        assert_eq!(json["warnings"], serde_json::json!([]));
    }

    /// The shape both write commands (`set_bundle_enabled`, `remove_bundle`)
    /// return for a bare refusal: the **raw** code in `errorCode` plus the
    /// **localized** reason in `diagnostic` — the single rule the frontend
    /// renders. Driven through the same two helpers both commands compose
    /// (`ChangeOutcome::from` + `localize_refusal`).
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

        // Bundle toggle: `management-required` (the "ships with DSH" refusal).
        let bundle = localize_refusal(ChangeOutcome::from_native(refusal("management-required"), "zh-CN"));
        assert_eq!(bundle.error_code.as_deref(), Some("management-required"));
        assert_eq!(
            bundle.diagnostic.as_deref(),
            Some("该条目由 DSH 自身管理，不能在这里改动")
        );

        // Bundle removal: `not-removable`.
        let removed = localize_refusal(ChangeOutcome::from_native(refusal("not-removable"), "zh-CN"));
        assert_eq!(removed.error_code.as_deref(), Some("not-removable"));
        assert_eq!(
            removed.diagnostic.as_deref(),
            Some("该 Bundle 随 DSH 发布，只能停用不能删除")
        );

        // A write naming a layer the profile does not have: `unknown-plugin`.
        let entry = localize_refusal(ChangeOutcome::from_native(refusal("unknown-plugin"), "zh-CN"));
        assert_eq!(entry.error_code.as_deref(), Some("unknown-plugin"));
        assert_eq!(
            entry.diagnostic.as_deref(),
            Some("实例里找不到这个插件")
        );

        // An empty code still reads as a sentence, never an empty diagnostic.
        let bare = localize_refusal(ChangeOutcome::from_native(refusal(""), "zh-CN"));
        assert_eq!(bare.error_code.as_deref(), Some(""));
        assert_eq!(bare.diagnostic.as_deref(), Some("实例未说明原因"));
    }

    /// A refusal is rewritten so the page shows the Chinese reason verbatim
    /// (`diagnostic` carries it; the code stays for logic) — the UI never maps
    /// codes. A real "changed but failed" keeps the instance's diagnostic.
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

        // changed:true is the instance's own "changed but didn't take effect":
        // its diagnostic must survive untouched.
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
            name: name.to_string(),
        }
    }

    /// Removal needs the profile's instance running: the native plugin manager
    /// owns plugin state (ADR-0001), so with none the command is a plain `Err`
    /// (not a refusal outcome) so the UI re-syncs and tells the user to start.
    #[tokio::test]
    async fn remove_bundle_refuses_without_a_running_instance() {
        let state = app_state(one_home(), one_instance());
        let err = remove_bundle_for(&state, &remove_input("ds-bot"))
            .await
            .unwrap_err();
        assert!(err.contains("未在运行"), "err: {err}");
    }

    /// An adopted instance has no recoverable token, so its bundles cannot be
    /// removed through the native API — a distinct message from "not running".
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
                },
            );
        }
        let err = remove_bundle_for(&state, &remove_input("ds-bot"))
            .await
            .unwrap_err();
        assert!(err.contains("接管"), "err: {err}");
    }
}
