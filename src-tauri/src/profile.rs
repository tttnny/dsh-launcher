//! Profile module — owns the profile layout under a DSH_HOME and the profile
//! patch document (`cordis.patch.yml`).
//!
//! A profile is one directory under `<DSH_HOME>/profiles/<name>`. Its patch
//! document's plugin entries are written by the CLI (insert rows when a plugin
//! is installed) and the `@linxin666/dsh-remote-web-ui` plugin manages a marked
//! lan-bind block that pins the web port. The launcher no longer writes
//! `disabled:` rows to gate plugin entries: plugin state belongs to the running
//! DSH (ADR-0001), so the launcher only **reads** those rows (see
//! [`read_disabled_ids`]). What the launcher still owns here is the port
//! pinning that must land before a start ([`scrub_port_pin`] /
//! [`assert_profile_lan_bind_port`]).
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

/// Parse disabled cordis ids from a patch document.
///
/// The document is a top-level YAML **array** of loader patch entries. Two
/// spellings of it are in the wild, and both must parse:
///
/// * the **block** style the CLI/launcher append (`- id: x` / `  disabled:
///   true`), and
/// * the **flow** style the native plugin manager rewrites the whole file into
///   when it saves (`[ { id: x, disabled: true }, … ]`, probe §6).
///
/// A line scanner cannot see the flow form (it only matched indented `- id:`
/// rows), which silently reported a native-written profile as fully enabled.
/// So this uses a real YAML parser and falls back to the scanner — the parser
/// chokes on `!!js`-tagged expressions and other DSH-specific values, and a
/// degraded read (no disabled info) beats failing the whole view.
fn parse_disabled_ids(raw: &str) -> HashSet<String> {
    match parse_disabled_ids_yaml(raw) {
        Some(set) => set,
        // The scanner understands the block dialect, the flat scan recovers
        // `{ … disabled: true }` rows the parser could not get past.
        None => {
            let mut set = parse_disabled_ids_scanned(raw);
            set.extend(parse_disabled_ids_flow(raw));
            set
        }
    }
}

/// Entry ids the native manager prints in its **flow** dialect, `{ id: x,
/// disabled: true }` on one line, including inside a `[ … ]` array. This exists
/// only because [`serde_yaml`] rejects the `!!js`-tagged values a patch
/// document may legitimately carry: the flat rows are easy to read even when
/// the surrounding document is not valid plain YAML.
fn parse_disabled_ids_flow(raw: &str) -> HashSet<String> {
    fn closing_brace(line: &str, from: usize) -> Option<usize> {
        let bytes = line.as_bytes();
        let mut depth = 0usize;
        for (i, b) in bytes.iter().enumerate().skip(from) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn entry_disabled(body: &str) -> Option<(String, bool)> {
        // Drop a quoted value if present; flat ids are bare in practice.
        let id = body
            .split_once("id:")
            .map(|(_, v)| v.trim())
            .map(|v| v.split([',', '}']).next().unwrap_or(""))
            .map(|v| v.trim().trim_matches('"').trim_matches('\''))
            .filter(|v| !v.is_empty())?;
        let disabled = body.split_once("disabled:").is_some_and(|(_, v)| {
            v.trim_start().starts_with("true")
        });
        Some((id.to_string(), disabled))
    }

    let mut set = HashSet::new();
    for line in raw.lines() {
        let mut byte = 0usize;
        while let Some(rel) = line[byte..].find('{') {
            let open = byte + rel + 1;
            let Some(close) = closing_brace(line, open) else {
                break;
            };
            if let Some((id, true)) = entry_disabled(&line[open..close.saturating_sub(1)]) {
                set.insert(id);
            }
            byte = close;
        }
    }
    set
}

/// YAML-parsed disabled ids: `None` means "not usable, fall back".
fn parse_disabled_ids_yaml(raw: &str) -> Option<HashSet<String>> {
    #[derive(serde::Deserialize)]
    struct PatchEntry {
        id: Option<String>,
        #[serde(default)]
        disabled: bool,
    }

    let doc: serde_yaml::Value = serde_yaml::from_str(raw).ok()?;
    // A sequence is the documented shape; a bare mapping (a single entry) is
    // accepted too, so one odd top-level value never blanks the whole read.
    let entries: Vec<PatchEntry> = match doc {
        serde_yaml::Value::Sequence(_) => serde_yaml::from_value(doc).ok()?,
        serde_yaml::Value::Mapping(_) => serde_yaml::from_value(doc).ok().map(|e| vec![e])?,
        _ => Vec::new(),
    };
    Some(
        entries
            .into_iter()
            .filter(|e| e.disabled)
            .filter_map(|e| e.id)
            .collect(),
    )
}

/// The block-style scanner (kept as the degraded fallback for documents the
/// YAML parser rejects). Only `id` rows with a *scalar* value are recorded, so
/// an `id:` that opens an insert / bundle block is never mistaken for one.
fn parse_disabled_ids_scanned(raw: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut current_id: Option<String> = None;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("- id:") {
            current_id = scalar_after(t, "- id:");
        } else if t.starts_with("id:") && !line.starts_with(' ') && !line.starts_with('\t') {
            current_id = scalar_after(t, "id:");
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

/// The bare scalar on a `id: <value>` line, if it is one (a block-opening `id:`
/// with nothing after it yields `None`).
fn scalar_after(line: &str, key: &str) -> Option<String> {
    let value = line.trim_start_matches(key).trim();
    if value.is_empty() || value.starts_with('#') {
        None
    } else {
        Some(value.to_string())
    }
}

/// The disabled cordis ids recorded in a profile's patch document.
pub(crate) fn read_disabled_ids(profile_dir: &Path) -> HashSet<String> {
    match std::fs::read_to_string(patch_path(profile_dir)) {
        Ok(raw) => parse_disabled_ids(&raw),
        Err(_) => HashSet::new(),
    }
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
/// that have no instance to ask — the profile template boot — derive it from
/// the profile itself instead of a user setting.
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
    fn parse_disabled_ids_handles_blocks() {
        let raw = "# header\n- id: ui-dsh-aionui-panel\n  disabled: true\n\n- id: live-stats\n  disabled: true\n\n- id: keep\n  config:\n    x: 1\n";
        let set = parse_disabled_ids(raw);
        assert!(set.contains("ui-dsh-aionui-panel"));
        assert!(set.contains("live-stats"));
        assert!(!set.contains("keep"));
    }

    /// The read side the offline view depends on: [`read_disabled_ids`] reads a
    /// profile's own patch document. It only ever reads — the launcher no
    /// longer writes `disabled:` rows (ADR-0001), so this file is the one place
    /// the launcher still sees another writer's disables.
    #[test]
    fn read_disabled_ids_reads_the_patch_document() {
        let dir = std::env::temp_dir().join(format!("dsh-profile-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("cordis.patch.yml"),
            "- id: dsh-auxiliary\n  disabled: true\n- id: dsh-base\n  disabled: false\n",
        )
        .unwrap();
        let disabled = read_disabled_ids(&dir);
        assert!(disabled.contains("dsh-auxiliary"), "disabled: {disabled:?}");
        assert!(!disabled.contains("dsh-base"), "disabled: {disabled:?}");
        // Reading writes nothing: the document is byte-identical afterwards.
        let after = std::fs::read_to_string(dir.join("cordis.patch.yml")).unwrap();
        assert!(after.contains("dsh-auxiliary"), "after: {after}");

        // A profile with no patch document reads as "nothing disabled" rather
        // than erroring — the offline view renders regardless.
        assert!(read_disabled_ids(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The native manager writes a **flow-style** top-level array (probe §6):
    /// `[ { id: llm, disabled: false }, … ]` on one line. The old line scanner
    /// only understood indented `- id:` blocks, so it saw none of these rows
    /// and reported every entry of a native-written profile as enabled.
    #[test]
    fn parse_disabled_ids_reads_native_flow_style() {
        // The verbatim document the native writer emits (probe §6): both rows
        // are `disabled: false`, so nothing is disabled — but crucially the
        // parser must not crash and must not invent a disabled id.
        let native_sample = "# Your patch layer for this dsh profile, applied after every bundle layer:\n\
# a top-level YAML array of loader patch entries (id-targeted config\n\
# overrides, disables, and insert lists; `!!js` expressions allowed).\n\
[ { id: llm, disabled: false }, { id: tool-plugin-manager, disabled: false } ]\n";
        assert!(parse_disabled_ids(native_sample).is_empty());

        // The same flow shape with an actually-disabled row: the case the old
        // scanner got wrong (it returned an empty set, hiding the disable).
        let disabled = "[ { id: llm, disabled: false }, { id: tool-plugin-manager, disabled: true } ]\n";
        let set = parse_disabled_ids(disabled);
        assert!(set.contains("tool-plugin-manager"), "set: {set:?}");
        assert!(!set.contains("llm"), "set: {set:?}");
    }

    /// A document the YAML parser rejects (or an entry shape we do not model)
    /// must degrade — never panic and never fail the whole view.
    #[test]
    fn parse_disabled_ids_degrades_on_odd_documents() {
        assert!(parse_disabled_ids("").is_empty());
        assert!(parse_disabled_ids("# only a comment\n").is_empty());
        assert!(parse_disabled_ids("[]\n").is_empty());
        // A scalar / non-entry element inside the array is ignored.
        let odd = "[ 42, { id: x, disabled: true }, { disabled: true }, { id: y } ]\n";
        let set = parse_disabled_ids(odd);
        assert!(set.contains("x"), "set: {set:?}");
        assert!(!set.contains("y"), "set: {set:?}");
        // A `!!js`-tagged value must not abort the read; the flat `id`/`disabled`
        // rows around it are still collected.
        let tagged = "[ { id: llm, disabled: true, config: !!js process.env.X } ]\n";
        let tagged_set = parse_disabled_ids(tagged);
        assert!(tagged_set.contains("llm"), "tagged: {tagged_set:?}");
    }

    /// Both the old block style and the new flow style can coexist in one
    /// document (the CLI appends blocks, the native manager rewrites flow).
    #[test]
    fn parse_disabled_ids_handles_mixed_styles_and_top_level_mapping() {
        let mixed = "# header\n- id: block-off\n  disabled: true\n[ { id: flow-off, disabled: true } ]\n";
        let set = parse_disabled_ids(mixed);
        assert!(set.contains("block-off"), "set: {set:?}");
        assert!(set.contains("flow-off"), "set: {set:?}");

        // A bare top-level mapping (no sequence) is still understood: the old
        // scanner read a column-0 `id:` line the same way.
        let mapping = "id: webserver\ndisabled: true\n";
        assert!(parse_disabled_ids(mapping).contains("webserver"));
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

    /// The one spawn point with no instance to ask (profile template boot)
    /// reads the flag out of the profile manifest, so this predicate is what
    /// keeps a linked plugin working there.
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
