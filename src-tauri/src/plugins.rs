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

    let mut ids: Vec<String> = Vec::new();
    let mut versions: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    if let Some(deps) = manifest.get("dependencies").and_then(|d| d.as_object()) {
        for (name, spec) in deps {
            if name.starts_with("@deepseek-ai/") {
                continue;
            }
            ids.push(name.clone());
            versions.insert(name.clone(), spec.as_str().unwrap_or("").to_string());
        }
    }
    if let Some(bundles) = manifest
        .pointer("/dsh/profile/bundles")
        .and_then(|b| b.as_array())
    {
        for b in bundles {
            if let Some(name) = b.as_str() {
                if name.starts_with("@deepseek-ai/") || ids.iter().any(|i| i == name) {
                    continue;
                }
                ids.push(name.to_string());
            }
        }
    }
    ids.sort();
    ids.dedup();

    // Disabled set from cordis.patch.yml (`- id: <cordis-id>` + `disabled: true`).
    let disabled = crate::profile::read_disabled_ids(&dir);

    let out = ids
        .into_iter()
        .map(|id| {
            let cordis_id = cordis_id_of(&id);
            let enabled = !disabled.contains(&cordis_id) && !disabled.contains(&id);
            InstalledPlugin {
                version: versions.get(&id).cloned(),
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

    let node = crate::runtime::node_for_spawn_checked(state).await?;
    let pnpm_prog = crate::toolchain::ensure_pnpm().await?;
    let what = format!("dsh plugin {subcommand}");

    // A node_modules tree linked from a *different* pnpm store makes pnpm
    // fail with ERR_PNPM_UNEXPECTED_STORE; relink proactively like the
    // installer path did.
    let store_dir = crate::toolchain::store_dir(&state.data_dir);
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
}
