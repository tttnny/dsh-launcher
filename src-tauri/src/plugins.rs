// Profile plugins: the instance-native plugin commands (list / enable /
// disable / remove) over the running instance's own `/api/pluginManager/*`
// service, plus the profile-file preparation the launcher's own pnpm needs
// (build scripts, peer policy, pnpm store pin). The launcher never writes
// plugin state — the running DSH owns it (ADR-0001).

use crate::config::{DshHome, Host};
use crate::native::{
    BundleItem, NativeChangeResult, NativeClient, NativeTransport, PluginView, PluginViewMode,
    ReqwestTransport,
};
use crate::AppState;
use serde::Deserialize;
use std::sync::Arc;
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

/// The transport the plugin client talks over in production: HTTP to the address
/// the running table holds (ADR-0001 — the running DSH owns plugin state). The
/// `_via` entry points take the transport instead, so a test drives every page
/// state through the plugin client's own fake without a live instance; which
/// address the page talks to, and what it says when it cannot, is the same
/// either way.
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

/// The body of [`list_plugins`], taking `&AppState` so tests can drive the
/// read-only states without a Tauri runtime. It builds the HTTP transport
/// production uses; [`list_plugins_via`] is the same read over a transport the
/// caller hands in.
async fn list_plugins_for(
    state: &AppState,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
) -> Result<PluginView, String> {
    list_plugins_via(state, http_transport()?, home_id, profile, instance_id).await
}

/// The read itself: which address the page talks to, and what it says when it
/// cannot talk to one at all.
async fn list_plugins_via(
    state: &AppState,
    transport: Arc<dyn NativeTransport>,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
) -> Result<PluginView, String> {
    // The read-only reasons are distinct on purpose: the UI shows them
    // verbatim, so each one has to say what happened and what to do.
    const NOT_RUNNING: &str = "实例未在运行：启动该实例后即可在这里管理它的补丁层。";
    const ADOPTED: &str =
        "该实例由系统接管（DSH 自行重启），没有可用的启动 token，无法管理补丁层。请从启动器重启该实例。";

    let (home, host) = home_and_host(state, home_id)?;

    // Nothing on that machine can be asked while the launcher cannot reach it:
    // not the instance, and not the profile files either. Saying so is the whole
    // answer — asking that machine again is the page's 重试 button — and it is
    // why this does not fall through to a file projection: those files are over
    // there, and a local `fs` call aimed at that path answers about this Mac
    // (ADR-0006).
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
        // No instance to ask, but the page still answers the question the user
        // came with ("这个 Profile 装了什么？") from the profile's own files,
        // read where they live.
        let bundles = profile_projection(state, &host, &home, profile).await;
        return Ok(PluginView::readonly(
            PluginViewMode::NotRunning,
            NOT_RUNNING,
            bundles,
        ));
    };
    // What decides is the token, not how we came to own the entry: an adopted
    // process whose launch URL we remembered is as manageable as a child of
    // ours, and the bare URL of one we cannot authenticate is not.
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

    // The address comes from the running table's own rule rather than from the
    // entry as it stands: a remote instance's entry holds the address its port
    // forward had when it was written, and a forward dies with the ssh
    // connection under it (ADR-0008). Asking here re-confirms or rebuilds it, so
    // a plugins page opened after this Mac slept reaches the instance instead of
    // failing to connect; 本机's instances answer with their own URL, unchanged.
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
    // A refused session / unreachable instance is read-only rather than a
    // command error: the page still renders with the instance's reason.
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

/// The DSH_HOME record and the machine that owns it. Every path in it is a path
/// **on that machine** — a remote host's DSH_HOME, its profile files and its
/// instance all live over there (ADR-0006).
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

/// What the page says when the machine that owns the DSH_HOME cannot be reached:
/// the machine's own reason when it gave one, and the action that can change the
/// answer. `None` when it has not been reported unreachable — the predicate and
/// the reason are one question, asked in one place
/// ([`AppState::host_unreachable_reason`]), so two callers cannot disagree
/// about the same machine.
fn host_unreachable_note(state: &AppState, host: &Host) -> Option<String> {
    let reason = state.host_unreachable_reason(&host.id)?;
    Some(format!(
        "主机「{}」现在连不上（{reason}）。那台机器上的实例问不到，Profile 文件也读不到；恢复连接后点「重试」。",
        host.display_name
    ))
}

/// The profile's own files as a layer list, read **where they live**: 本机's
/// profile is read with `std::fs`, a remote host's in one call over the
/// transport. A local `fs` call aimed at a remote path would answer about this
/// Mac (ADR-0006), and one call rather than one per package is what keeps the
/// fallback cheap — this projection is used exactly when nothing is running.
///
/// Best effort in both directions: an absent profile, an unreadable manifest
/// and a machine that does not answer all project to nothing, and the page says
/// which state it is in from its own reason.
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

/// One profile's own files as a layer list, without a filesystem: `manifest` is
/// that `package.json`'s text as it was read where it lives, and `version_of`
/// answers the installed version of one package name — 本机 reads it out of
/// `node_modules/<name>/package.json`, a remote host already has them from the
/// one call that fetched the profile's files.
///
/// Read-only by construction — it never writes a profile file (ADR-0001) — and
/// deliberately thinner than the live list: it cannot know live state,
/// availability, removal rules or a layer's row count, so it reports only what
/// the files say, and the page labels it as such.
///
/// The order is the profile's own: the bundles it selects (which is its
/// priority order), then the packages it installs but does not mount.
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

    // The profile's order first — it is the priority order — then whatever else
    // the profile installs, so the same set always reads the same way.
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

/// [`offline_bundles_from`] against a HOME path on 本机, so tests need no
/// `AppState` config.
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

/// The version of an installed package, read from the copy pnpm linked into the
/// profile's `node_modules`. Absent when it is not there (or not readable),
/// which is exactly the case the page shows as "no version".
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
    /// The instance the action was taken from; it wins when it is running this
    /// profile (see [`running_for_profile`](crate::process::running_for_profile)).
    #[serde(default)]
    pub instance_id: Option<String>,
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
        input.instance_id.as_deref(),
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
    /// The instance the action was taken from (see [`SetBundleEnabledInput`]).
    #[serde(default)]
    pub instance_id: Option<String>,
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

/// [`linked_store_dir`] over the document itself, for a caller that read it from
/// the machine that owns the profile.
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
    store_target_from(linked_store_dir(profile_dir).as_deref(), launcher_store)
}

/// [`profile_store_target`] over the linked store the caller already has (read
/// from the machine that owns the profile): the same decision, one step further
/// from the file.
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

/// The document a profile's `pnpm-workspace.yaml` should hold so every pnpm
/// running in it agrees on `store_dir`, or `None` when it already says that.
///
/// `raw` is the file's text as it is now; `None` means the file is not there,
/// which gets the same base document DSH's own `initProfile` writes — that
/// writer never touches an existing file, so a thinner one created here would
/// permanently deny the profile its `nodeLinker: hoisted` (the setting
/// out-of-tree plugins depend on).
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
        return None;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Some(out)
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

/// The document a profile's `.npmrc` should hold, or `None` when it already
/// says it.
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
        // Keep a short rationale in the file: it is user-visible state.
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

/// Make sure a directory's pnpm-workspace.yaml opts into dependency build
/// scripts on every pnpm the launcher accepts: `onlyBuiltDependencies` for
/// pnpm 10, `allowBuilds` for 11 and 12.
///
/// The two newer majors differ in how an approval is spelled. pnpm 11 writes
/// `allowBuilds: <name>: set this to true or false` for every dependency whose
/// build script it ignored and then fails with ERR_PNPM_IGNORED_BUILDS; a
/// rewrite of that file turns the placeholders into `true`. pnpm 12 writes
/// nothing at all — it only reports the names — and takes no wildcard, so the
/// names `approve` carries, read off its own report, are the only approval it
/// accepts (issue #14 验收). Both spells are the same document, so a subsequent
/// install runs the native build scripts (node-pty, koffi, esbuild, sharp, …)
/// whichever major runs it.
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

/// The document [`ensure_build_scripts_allowed`] writes, from the one that is
/// already there — the whole decision, without touching a filesystem. `None`
/// when the file already says what it should, so a caller only writes when
/// there is something to write.
///
/// Split out because the same document has to be produced for a version
/// directory on another machine, where the launcher reads and writes over the
/// transport instead of `std::fs`; one function, so the two cannot disagree
/// about what an allowed build script looks like.
pub(crate) fn build_scripts_manifest(raw: &str, approve: &[String]) -> Option<String> {
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

    // 4. Name the packages pnpm reported and refused. This is the whole of the
    //    approval on pnpm 12 — it has no wildcard, so a name pnpm did not print
    //    is a name the launcher cannot guess — and it is harmless on pnpm 11,
    //    where the same section already carries pnpm's own placeholders.
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

/// The lines [`build_scripts_manifest`] may write approved package names into:
/// inside the `allowBuilds:` block, before the next top-level key (or the blank
/// line it keeps at the end of the document, which belongs with the newline).
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

/// The package names the `allowBuilds:` block already carries — pnpm 11's own
/// placeholders included, which step 1 of [`build_scripts_manifest`] has
/// already turned into an approval by the time this is asked.
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

/// Resolves the instance that can manage `(home_id, profile)`'s plugins, as a
/// connected [`NativeClient`](crate::native::NativeClient). `instance_id` is the
/// instance the page was opened from and wins when it is running that profile.
/// The refusal messages are the ones the page shows for the "cannot manage now"
/// states.
pub(crate) async fn write_client(
    state: &AppState,
    home_id: &str,
    profile: &str,
    instance_id: Option<&str>,
) -> Result<crate::native::NativeClient, String> {
    write_client_via(state, http_transport()?, home_id, profile, instance_id).await
}

/// [`write_client`] over a transport the caller hands in: the same resolution,
/// so a test can assert the address a write is aimed at without a live instance.
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
    // Same rule as the read path: a token is what makes the instance
    // addressable, however the entry came to exist.
    if crate::process::manageable_url(&entry).is_none() {
        return Err(if entry.adopted {
            "该实例由系统接管，无法管理插件".to_string()
        } else {
            "实例未在运行，请先启动实例".to_string()
        });
    }
    // Where the write goes is the same decision the read makes, and it is made
    // in the same place: the forwarded address for a remote instance, rebuilt if
    // the connection under it went away (ADR-0008).
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
        ensure_build_scripts_allowed(&dir, &[]).unwrap();
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
        ensure_build_scripts_allowed(&dir, &[]).unwrap();
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
        ensure_build_scripts_allowed(&dir, &[]).unwrap();
        let after = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")).unwrap();
        assert_eq!(before, after, "must be idempotent");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// pnpm 12 has no wildcard to approve with: `allowBuilds: {'*': true}` and
    /// `onlyBuiltDependencies: ['*']` both leave every build script refused, and
    /// what pnpm prints in `Ignored build scripts: …` is the only list of names
    /// there is. Those names, written here, are what an install on pnpm 12
    /// needs (issue #14 验收).
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

        // Handing the same names again is not a change: the caller only writes
        // when there is something to write.
        assert!(
            build_scripts_manifest(&document, &approve).is_none(),
            "已批准过的名字不该再写一遍"
        );

        // A second round names a package the first one did not, and it joins the
        // same block — the file pnpm has to read stays one document.
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
        app_state_with(
            vec![crate::config::Host::local()],
            homes,
            instances,
            crate::remote::testing::idle(),
        )
    }

    /// The state one test runs against, with the machines it knows and the
    /// transport the remote half of it dials: a host that is not this Mac has
    /// its DSH_HOMEs, its profile files and its instances over there, so every
    /// test that reads one drives a fake instead of a server.
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

    /// Without a running instance the page is read-only: the reason says what
    /// to do, and the list is the profile's own files — what it selects (its
    /// priority order first) and what it installs, with nothing invented that
    /// only a running instance could know.
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
        // The reason is distinct and tells the user what to do.
        let reason = view.reason.expect("read-only reason");
        assert!(reason.contains("启动该实例"), "reason: {reason}");

        // The profile's own order first, then the rest of what it installs.
        let names: Vec<&str> = view.bundles.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["@deepseek-ai/dsh-base", "@dsh-plugin/dsh-auxiliary"]
        );
        assert!(view.bundles.iter().all(|b| b.enabled && b.installed));
        // Nothing a file cannot prove: no row counts, no removal rule, no
        // version (nothing is linked into node_modules in this scratch HOME).
        assert!(view.bundles.iter().all(|b| b.row_count == 0
            && !b.removable
            && b.version.is_none()
            && b.availability.is_none()
            && b.error_code.is_none()));
    }

    /// The file projection is a projection of the files, nothing else: a
    /// package the profile installs but does not mount shows up unmounted, and
    /// an unreadable/absent profile answers with no list at all.
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
        // A linked package with a version on disk is the one thing the files can
        // add; here ds-bot has no node_modules copy, so it gets none.
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

        // Nothing to read: an empty profile, and a profile that does not exist.
        assert!(offline_bundles_in(&home, "nope").is_empty());
        std::fs::write(dir.join("package.json"), "not json").unwrap();
        assert!(offline_bundles_in(&home, "web").is_empty());
    }

    /// An adopted instance carries a bare URL and no token, so the view is
    /// `adopted` — distinct from `not-running` — and its reason names the
    /// adoption, not "start the instance".
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
        // The HOME has no profile yet: the file projection simply has nothing.
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
            Vec::new(),
        );
        assert_ne!(Some(adopted_reason), not_running.reason);
    }

    /// Adoption is not what blocks management: when the launcher remembered
    /// the adopted process's launch URL, the write path goes on to connect
    /// like any other. Only the connection itself may then fail.
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
                    // Port 1 refuses immediately: the failure has to come from
                    // the connection, not from a refusal to try.
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

    /// An adopted instance has no recoverable token, so its bundles cannot be
    /// managed through the native API — a distinct message from "not running".
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
        let bundle = localize_refusal(ChangeOutcome::from_native(
            refusal("management-required"),
            "zh-CN",
        ));
        assert_eq!(bundle.error_code.as_deref(), Some("management-required"));
        assert_eq!(
            bundle.diagnostic.as_deref(),
            Some("该条目由 DSH 自身管理，不能在这里改动")
        );

        // Bundle removal: `not-removable`.
        let removed = localize_refusal(ChangeOutcome::from_native(
            refusal("not-removable"),
            "zh-CN",
        ));
        assert_eq!(removed.error_code.as_deref(), Some("not-removable"));
        assert_eq!(
            removed.diagnostic.as_deref(),
            Some("该 Bundle 随 DSH 发布，只能停用不能删除")
        );

        // A write naming a layer the profile does not have: `unknown-plugin`.
        let entry = localize_refusal(ChangeOutcome::from_native(
            refusal("unknown-plugin"),
            "zh-CN",
        ));
        assert_eq!(entry.error_code.as_deref(), Some("unknown-plugin"));
        assert_eq!(entry.diagnostic.as_deref(), Some("实例里找不到这个插件"));

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
            instance_id: None,
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
                    remote: None,
                },
            );
        }
        let err = remove_bundle_for(&state, &remove_input("ds-bot"))
            .await
            .unwrap_err();
        assert!(err.contains("接管"), "err: {err}");
    }

    // -----------------------------------------------------------------------
    // Remote hosts: the same page, over the launcher's own port forward
    // -----------------------------------------------------------------------

    use crate::native::testing::{
        ok_response, FakeTransport as PluginServer, COOKIE, INSTANCE_URL,
    };
    use crate::remote::testing::{FakeAnswer as HostAnswer, FakeCall, FakeTransport as HostServer};

    const TARGET: &str = "zjq_aliyun";
    const TOKEN: &str = "JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";
    /// Where that machine keeps its launcher data, and the DSH_HOME under it.
    const REMOTE_DATA: &str = "/home/zhou/.local/share/dsh-launcher";
    const REMOTE_HOME: &str = "/home/zhou/.local/share/dsh-launcher/homes/1.2.3";

    /// A machine that is not this Mac: an ssh target, and a data directory of
    /// its own over there.
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

    /// One running entry, as the running table holds it. `remote` is what makes
    /// it a remote instance: DSH printed an address on its own machine, and the
    /// launcher's table remembers where this Mac reaches it (`url`).
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

    /// The remote half of an entry: the machine, the address DSH printed there
    /// (the forward's target), and the pid that would be stopped there.
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

    /// That machine's own address for the instance — deliberately not loopback,
    /// so a request that went there instead of through the forward is visible.
    fn remote_url() -> String {
        format!("http://192.0.2.7:3080/?token={TOKEN}")
    }

    /// An address the entry may still be holding from a forward that has since
    /// died: a loopback port nothing answers on.
    fn stale_local_url() -> String {
        format!("http://127.0.0.1:9999/?token={TOKEN}")
    }

    async fn put_running(state: &AppState, id: &str, entry: crate::process::RunningInstance) {
        state.running.lock().await.insert(id.to_string(), entry);
    }

    /// The forwards the page asked that machine's transport for, in order.
    fn forwarded(fake: &HostServer) -> Vec<crate::ssh::PortForward> {
        fake.calls()
            .into_iter()
            .filter_map(|call| match call {
                FakeCall::OpenForward { forward, .. } => Some(forward),
                _ => None,
            })
            .collect()
    }

    /// A running remote instance is listed exactly as a local one is, over the
    /// address **this Mac** reaches it at: the page goes through the launcher's
    /// port forward (ADR-0008), so what the client talks to is loopback with the
    /// launch token — never the address DSH printed on that machine, which on
    /// this Mac is whoever happens to hold that port.
    ///
    /// The address is not taken from the entry as it stands either: a forward
    /// dies with the ssh connection under it (this Mac sleeping is enough), so
    /// the page re-confirms it and rebuilds it from that machine's own address.
    /// What the entry still holds from the dead forward is not what is used.
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

        // One forward, aimed at the address that machine printed…
        let forwards = forwarded(&hosts);
        assert_eq!(forwards.len(), 1, "一条实例地址一条转发");
        assert_eq!(forwards[0].remote_host, "192.0.2.7");
        assert_eq!(forwards[0].remote_port, 3080);
        assert_eq!(
            forwards[0].local_port, 3080,
            "本机端口与远端相同（没被占时）"
        );

        // …and the client talked to loopback on the port the forward gave it,
        // with the launch token intact.
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

    /// Adoption is not a refusal on the read path either
    /// (`docs/agents/rules/instances.md`): an adopted remote instance the
    /// launcher holds a launch token for is listed as managed, over the
    /// address this Mac reaches it at — the same rule the write path follows.
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

    /// A write goes exactly where the read goes. And adoption is not a refusal
    /// (`docs/agents/rules/instances.md`): what decides is the token, so an
    /// adopted remote instance the launcher holds a launch token for is written
    /// to over the forward and reported truthfully.
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

    /// A local instance is unchanged by all of this: the page talks to its own
    /// URL, straight from the running table, and asks no machine for a port
    /// forward.
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

    /// With no instance running, the fallback is that **machine's** own profile
    /// files — read in one call, because this projection is what the page shows
    /// exactly when nothing is running — and it stays as thin as a file can be:
    /// no row counts, no removal rule, no availability.
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

        // One call for the whole profile, aimed at that machine's own path.
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

    /// A machine the launcher cannot reach: the page says why, offers the one
    /// action that can change the answer, and asks that machine nothing more —
    /// a second ssh from a page load would only make the wait longer, and the
    /// profile files it would read are over there anyway.
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

    /// The other unreachable case: the machine answers, and the **tunnel** does
    /// not. That is a message about this Mac's connection to that machine — with
    /// the address to try by hand — and it must not read like DSH refusing the
    /// operation (ADR-0008).
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

    /// An instance whose URL carries no token is read-only, and its reason names
    /// the adoption — while the file projection still comes from **that**
    /// machine, in one call.
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
        // An adopted process with a bare address: it is running over there, and
        // nothing here can authenticate to it.
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

    /// No token is a refusal; a machine that is not there is a refusal of its
    /// own — and neither of them is "实例未在运行", which is what the page turns
    /// into a 启动 button (ADR-0007 后果: starting a second DSH on a server that
    /// was running one all along).
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
