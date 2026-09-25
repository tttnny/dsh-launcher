//! Profile module — owns the profile layout under a DSH_HOME and the profile
//! patch document (`cordis.patch.yml`).
//!
//! A profile is one directory under `<DSH_HOME>/profiles/<name>`. Its patch
//! document is edited by three parties: the CLI writes insert rows when a
//! plugin is installed, the launcher writes `disabled:` rows to gate plugins,
//! and the `@linxin666/dsh-remote-web-ui` plugin manages a marked lan-bind
//! block that pins the web port. Every editor of that document lives here so
//! the row-ownership rules ("who owns which rows") have one home.
//!
//! Path derivation goes through [`profile_dir`], which keeps a profile name
//! a single path segment — a traversal attempt can never escape `profiles/`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Layout & naming
// ---------------------------------------------------------------------------

/// The profiles directory under a DSH_HOME.
pub(crate) fn profiles_dir(home: &Path) -> PathBuf {
    home.join("profiles")
}

/// Whether a profile name is a single safe path segment (no separators, not
/// `.`/`..`). This is the traversal guard every profile path join passes
/// through; the UI-level naming policy (reserved names, charset) is
/// [`validate_profile_name`].
pub(crate) fn is_single_path_segment(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}

/// A profile directory under a HOME. Errors on names that are not a single
/// path segment, so callers can never join an escaping path.
pub(crate) fn profile_dir(home: &Path, profile: &str) -> Result<PathBuf, String> {
    if !is_single_path_segment(profile) {
        return Err(format!("Profile 名称不合法: {profile}"));
    }
    Ok(profiles_dir(home).join(profile))
}

/// Validates a user-chosen profile name (create/rename flows): charset plus
/// the reserved names the template/node_modules machinery owns.
pub(crate) fn validate_profile_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Profile 名称不能为空".to_string());
    }
    if name == "__temp__" || name == "node_modules" {
        return Err(format!("「{name}」为保留名称，不能使用"));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err("Profile 名称只能包含字母、数字、-、_、.".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The patch document (cordis.patch.yml)
// ---------------------------------------------------------------------------

/// Path of a profile's patch document.
fn patch_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("cordis.patch.yml")
}

/// cordis id for a package: bundles register under their unscoped short name
/// (dsh-auxiliary) unless the package declares otherwise. We default to the
/// last path segment without the scope.
pub fn cordis_id_of(package: &str) -> String {
    let last = package.rsplit('/').next().unwrap_or(package);
    last.to_string()
}

/// Marker lines the `@linxin666/dsh-remote-web-ui` plugin wraps its managed
/// lan-bind block in (must match that plugin's `lan-bind.ts`).
const LAN_BIND_BLOCK_BEGIN: &str = "# --- remote-web-ui lan-bind block (managed - do not edit) ---";
const LAN_BIND_BLOCK_END: &str = "# --- end remote-web-ui lan-bind block ---";

/// Parse disabled cordis ids from a patch document. We do a lightweight line
/// scan (avoid pulling a YAML parser dependency for this).
pub(crate) fn parse_disabled_ids(raw: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut current_id: Option<String> = None;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("- id:") {
            current_id = Some(t.trim_start_matches("- id:").trim().to_string());
        } else if t.starts_with("id:") && !line.starts_with(' ') && !line.starts_with('\t') {
            current_id = Some(t.trim_start_matches("id:").trim().to_string());
        } else if t == "disabled: true" {
            if let Some(id) = current_id.take() {
                set.insert(id);
            }
        } else if t.starts_with("- ") && !t.starts_with("- id:") {
            current_id = None;
        }
    }
    set
}

/// The disabled cordis ids recorded in a profile's patch document.
pub(crate) fn read_disabled_ids(profile_dir: &Path) -> HashSet<String> {
    match std::fs::read_to_string(patch_path(profile_dir)) {
        Ok(raw) => parse_disabled_ids(&raw),
        Err(_) => HashSet::new(),
    }
}

/// Add or remove a `disabled: true` row for a cordis id in the document.
pub(crate) fn set_disabled_row(raw: &str, cordis_id: &str, enabled: bool) -> String {
    // Remove any existing rows for this id (both plain and commented forms).
    let mut out: Vec<String> = Vec::new();
    let mut skip_block = false;
    for line in raw.lines() {
        let t = line.trim();
        // A top-level `[]` placeholder is dropped when we have any real entry
        // to write; it is kept only while the document stays empty.
        if t == "[]" {
            continue;
        }
        let is_target_id = t == format!("- id: {cordis_id}") || t == format!("id: {cordis_id}");
        if is_target_id {
            // Start of a block for this id; look ahead: if it is a pure
            // `disabled: true` block we drop it entirely.
            skip_block = true;
            continue;
        }
        if skip_block {
            // Inside the block: only `disabled:` and blank lines belong to it.
            if t == "disabled: true" || t == "disabled: false" || t.is_empty() {
                skip_block = false; // end of this small block
                continue;
            }
            // Block has other content (config etc.) — keep it, stop skipping.
            skip_block = false;
            out.push(line.to_string());
            continue;
        }
        out.push(line.to_string());
    }

    let mut cleaned: Vec<String> = out;
    // Trim trailing blank lines.
    while cleaned.last().map(|l| l.trim().is_empty()) == Some(true) {
        cleaned.pop();
    }

    if !enabled {
        // Append a fresh disable row (block sequence, never after `[]`).
        cleaned.push(String::new());
        cleaned.push(format!("- id: {cordis_id}"));
        cleaned.push("  disabled: true".to_string());
    }

    finalize_document(cleaned)
}

/// Strips every patch block whose id equals `cordis_id` (matching plain
/// `- id:` / `id:` rows, including `- insert:` wrappers) — used by uninstall,
/// where the CLI already removed the dependency.
pub(crate) fn strip_cordis_rows(raw: &str, cordis_id: &str, plugin_id: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut skip = false;
    for line in raw.lines() {
        let t = line.trim();
        if t == "[]" {
            continue;
        }
        // Start of a block for the target: `- id: <id>` (plain or insert row).
        let is_target = t == format!("- id: {cordis_id}")
            || t == format!("id: {cordis_id}")
            || t == format!("- id: {plugin_id}")
            || t == format!("id: {plugin_id}");
        if is_target {
            skip = true;
            continue;
        }
        if skip {
            // Inside a target block: drop indented child lines and blank
            // separators; stop at the next top-level key.
            if t.is_empty() {
                continue;
            }
            let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            if indent > 0 {
                continue;
            }
            skip = false;
        }
        out.push(line.to_string());
    }
    finalize_document(out)
}

/// Joins edited lines back into a document: trim trailing blanks, ensure a
/// trailing newline, and restore the `[]` placeholder when no real rows
/// remain (the file must stay a valid top-level array).
fn finalize_document(mut lines: Vec<String>) -> String {
    while lines.last().map(|l| l.trim().is_empty()) == Some(true) {
        lines.pop();
    }
    let mut result = lines.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    let body: String = result
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n");
    if body.trim().is_empty() {
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str("[]\n");
    }
    result
}

/// Rewrites the webserver `port:` row inside the `id: webserver` block to `0`
/// (OS-assigned random port). DSH persists the first web bind into the patch
/// document and honors it over the CLI `--port`, so a template carrying a
/// concrete port would make every instance derived from it fight over that
/// port (EADDRINUSE) and ignore the launcher's per-instance `--port`.
pub(crate) fn scrub_port_pin(raw: &str) -> String {
    rewrite_row_in_id_block(raw, "webserver", "port", "0").0
}

/// Rewrites the port pinned by the managed lan-bind block so the launcher's
/// `--port` takes effect on the very next start.
///
/// That block is a top-level patch row and outranks the CLI `--port`, while
/// the plugin only re-asserts it at boot — after the web server has already
/// bound. Without this nudge the first start after a port change comes up on
/// the old port and only the following start uses the new one. An absent
/// block is left untouched: an unmanaged profile carries no port override, so
/// the CLI flag already wins there.
pub(crate) fn set_lan_bind_port(raw: &str, port: u16) -> String {
    let mut in_block = false;
    let mut changed = false;
    let lines: Vec<String> = raw
        .lines()
        .map(|l| {
            let trimmed = l.trim_start();
            if trimmed.starts_with(LAN_BIND_BLOCK_BEGIN) {
                in_block = true;
            } else if trimmed.starts_with(LAN_BIND_BLOCK_END) {
                in_block = false;
            } else if in_block && trimmed.starts_with("port:") {
                let indent = " ".repeat(l.len() - trimmed.len());
                let new = format!("{indent}port: {port}");
                if new != l {
                    changed = true;
                }
                return new;
            }
            l.to_string()
        })
        .collect();
    if !changed {
        return raw.to_string();
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Rewrites `<key>: <value>` rows inside the `- id: <block_id>` block of a
/// patch document, preserving indentation. Returns the new document and
/// whether anything changed.
fn rewrite_row_in_id_block(raw: &str, block_id: &str, key: &str, value: &str) -> (String, bool) {
    let id_row = format!("- id: {block_id}");
    let key_row = format!("{key}:");
    let mut in_block = false;
    let mut changed = false;
    let lines: Vec<String> = raw
        .lines()
        .map(|l| {
            let trimmed = l.trim_start();
            if trimmed.starts_with("- id:") {
                in_block = trimmed.starts_with(&id_row);
            } else if in_block && trimmed.starts_with(&key_row) {
                let indent = " ".repeat(l.len() - trimmed.len());
                let new = format!("{indent}{key}: {value}");
                if new != l {
                    changed = true;
                }
                return new;
            }
            l.to_string()
        })
        .collect();
    if !changed {
        return (raw.to_string(), false);
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    (out, true)
}

// ---------------------------------------------------------------------------
// File-level patch operations
// ---------------------------------------------------------------------------

/// Whether a profile's manifest declares any `link:` dependency.
///
/// A linked plugin needs DSH's own kernel-package routing, which keys on the
/// importer living inside the profiles directory — that is what
/// `--preserve-symlinks` preserves (see
/// [`crate::launch::node_runtime_flags`]). The flag cannot be left on for
/// every process though: it also makes DSH load `@deepseek-ai/dsh-app-boot`
/// more than once and every settings write then fails. So the spawn points
/// that have no instance to ask — the profile template boot and
/// `dsh plugin` — derive it from the profile itself instead of a user
/// setting.
///
/// Returns false for a missing or unreadable manifest: that is the common
/// case (`pnpm install` has not written it yet) and the safe default.
pub(crate) fn declares_link_dependency(profile_dir: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(profile_dir.join("package.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    manifest
        .get("dependencies")
        .and_then(|deps| deps.as_object())
        .is_some_and(|deps| {
            deps.values()
                .any(|spec| spec.as_str().is_some_and(|s| s.starts_with("link:")))
        })
}

/// Scrubs the webserver port pin in a copied profile directory's patch
/// document (template flows). Missing file / absent block: no-op.
pub(crate) fn scrub_profile_port_pin(profile_dir: &Path) {
    let patch = patch_path(profile_dir);
    let Ok(raw) = std::fs::read_to_string(&patch) else {
        return;
    };
    let scrubbed = scrub_port_pin(&raw);
    if scrubbed != raw {
        let _ = std::fs::write(&patch, scrubbed);
    }
}

/// Re-asserts the launcher's per-instance port in a profile's managed
/// lan-bind block before a start. Missing file / absent block: no-op.
pub(crate) fn assert_profile_lan_bind_port(home: &Path, profile: &str, port: u16) {
    let Ok(dir) = profile_dir(home, profile) else {
        return;
    };
    let patch = patch_path(&dir);
    let Ok(raw) = std::fs::read_to_string(&patch) else {
        return;
    };
    let updated = set_lan_bind_port(&raw, port);
    if updated != raw {
        let _ = std::fs::write(&patch, updated);
    }
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
    fn set_disabled_row_adds_and_removes() {
        let raw = "# comment\n- id: other-plugin\n  config:\n    a: 1\n";
        // Add a disable row for dsh-auxiliary.
        let out = set_disabled_row(raw, "dsh-auxiliary", false);
        assert!(out.contains("- id: dsh-auxiliary"), "out: {out}");
        assert!(out.contains("  disabled: true"), "out: {out}");
        // The unrelated block must be preserved.
        assert!(out.contains("other-plugin"), "out: {out}");
        assert!(out.contains("config"), "out: {out}");
        assert!(out.contains("a: 1"), "out: {out}");

        // Remove it again -> back to the original content.
        let back = set_disabled_row(&out, "dsh-auxiliary", true);
        assert!(!back.contains("dsh-auxiliary"), "back: {back}");
        assert!(back.contains("other-plugin"), "back: {back}");
        assert!(back.contains("config"), "back: {back}");
    }

    #[test]
    fn set_disabled_row_replaces_existing() {
        let raw = "- id: dsh-auxiliary\n  disabled: true\n";
        let out = set_disabled_row(raw, "dsh-auxiliary", true);
        assert!(!out.contains("dsh-auxiliary"), "out: {out}");
        // Re-disable after removal.
        let out2 = set_disabled_row(&out, "dsh-auxiliary", false);
        assert!(out2.contains("- id: dsh-auxiliary"), "out2: {out2}");
        assert!(out2.contains("  disabled: true"), "out2: {out2}");
    }

    #[test]
    fn parse_disabled_ids_handles_blocks() {
        let raw = "# header\n- id: ui-dsh-aionui-panel\n  disabled: true\n\n- id: live-stats\n  disabled: true\n\n- id: keep\n  config:\n    x: 1\n";
        let set = parse_disabled_ids(raw);
        assert!(set.contains("ui-dsh-aionui-panel"));
        assert!(set.contains("live-stats"));
        assert!(!set.contains("keep"));
    }

    #[test]
    fn strip_cordis_rows_removes_insert_and_disabled_blocks() {
        // A plugin mounted via an insert row plus a disabled row for another
        // plugin must leave the other plugin intact.
        let raw = "# header\n- insert:\n    - id: dsh-auxiliary\n      name: '@dsh-plugin/dsh-auxiliary'\n\n- id: dsh-thought-buddy\n  disabled: true\n\n- id: keep\n  config:\n    x: 1\n";
        let out = strip_cordis_rows(raw, "dsh-auxiliary", "@dsh-plugin/dsh-auxiliary");
        assert!(!out.contains("dsh-auxiliary"), "insert row removed: {out}");
        assert!(out.contains("dsh-thought-buddy"), "other block kept: {out}");
        assert!(out.contains("keep"), "config block kept: {out}");
        assert!(out.contains("x: 1"), "config content kept: {out}");
    }

    #[test]
    fn strip_cordis_rows_restores_placeholder_when_empty() {
        let raw = "# header\n- id: dsh-auxiliary\n  disabled: true\n";
        let out = strip_cordis_rows(raw, "dsh-auxiliary", "@dsh-plugin/dsh-auxiliary");
        assert!(out.contains("[]"), "placeholder restored: {out}");
        assert!(!out.contains("dsh-auxiliary"), "entry removed: {out}");
    }

    #[test]
    fn scrub_port_pin_rewrites_webserver_block_only() {
        let raw = "- id: webserver\n  port: 8080\n- id: other\n  port: 9090\n";
        let out = scrub_port_pin(raw);
        assert!(out.contains("port: 0"), "out: {out}");
        assert!(out.contains("port: 9090"), "unrelated port kept: {out}");
    }

    #[test]
    fn scrub_port_pin_noop_without_block() {
        let raw = "- id: other\n  port: 9090\n";
        assert_eq!(scrub_port_pin(raw), raw);
    }

    #[test]
    fn set_lan_bind_port_rewrites_managed_block_only() {
        let raw = format!(
            "- id: webserver\n  port: 1\n{LAN_BIND_BLOCK_BEGIN}\nport: 8080\nhost: 0.0.0.0\n{LAN_BIND_BLOCK_END}\n"
        );
        let out = set_lan_bind_port(&raw, 1234);
        assert!(out.contains("port: 1234"), "out: {out}");
        assert!(out.contains("host: 0.0.0.0"), "sibling row kept: {out}");
        assert!(out.contains("port: 1"), "block outside managed area kept: {out}");
    }

    #[test]
    fn set_lan_bind_port_noop_without_markers() {
        let raw = "- id: webserver\n  port: 8080\n";
        assert_eq!(set_lan_bind_port(raw, 1234), raw);
    }

    #[test]
    fn profile_dir_rejects_traversal_names() {
        assert!(profile_dir(Path::new("/h"), "../evil").is_err());
        assert!(profile_dir(Path::new("/h"), "..").is_err());
        assert!(profile_dir(Path::new("/h"), "a/b").is_err());
        assert!(profile_dir(Path::new("/h"), "a\\b").is_err());
        assert!(profile_dir(Path::new("/h"), "").is_err());
        assert_eq!(
            profile_dir(Path::new("/h"), "web").unwrap(),
            Path::new("/h/profiles/web")
        );
    }

    /// The two spawn points with no instance to ask (profile template boot,
    /// `dsh plugin`) read the flag out of the profile manifest, so this
    /// predicate is what keeps a linked plugin working there.
    #[test]
    fn declares_link_dependency_reads_the_manifest() {
        let dir = std::env::temp_dir().join(format!("dsh-profile-test-{}", uuid::Uuid::new_v4()));
        // No manifest at all (pnpm install has not written one) -> false.
        assert!(!declares_link_dependency(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!declares_link_dependency(&dir));

        // Regular semver deps, and a manifest without a dependencies object.
        std::fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"@dsh-plugin/plain":"^0.3.1"}}"#,
        )
        .unwrap();
        assert!(!declares_link_dependency(&dir));
        std::fs::write(dir.join("package.json"), r#"{"private":true}"#).unwrap();
        assert!(!declares_link_dependency(&dir));

        // Unparsable JSON is treated as "no link deps" rather than an error:
        // the safe default, since the flag must not be enabled by accident.
        std::fs::write(dir.join("package.json"), "{ not json").unwrap();
        assert!(!declares_link_dependency(&dir));

        // A link: dep anywhere in the map turns it on.
        std::fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"@dsh-plugin/plain":"^0.3.1","my-plugin":"link:../my-plugin"}}"#,
        )
        .unwrap();
        assert!(declares_link_dependency(&dir));

        std::fs::remove_dir_all(&dir).ok();
    }
}
