//! Profile module — owns the profile layout under a DSH_HOME and the profile
//! patch document (`cordis.patch.yml`).
//!
//! A profile is one directory under `<DSH_HOME>/profiles/<name>`. Its patch
//! document's plugin entries are written by the CLI (insert rows when a plugin
//! is installed) and the `@linxin666/dsh-remote-web-ui` plugin manages a marked
//! lan-bind block that pins the web port. The launcher never writes plugin
//! state: patch layers and their entries belong to the running DSH (ADR-0001),
//! and the plugin page talks to that process over HTTP instead. What the
//! launcher still owns here is the one *read* fact a start needs — the port pin
//! that must land before launch ([`scrub_port_pin`] /
//! [`assert_profile_lan_bind_port`]).
//!
//! Path derivation goes through [`profile_dir`], which keeps a profile name
//! a single path segment — a traversal attempt can never escape `profiles/`.

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

/// Marker lines the `@linxin666/dsh-remote-web-ui` plugin wraps its managed
/// lan-bind block in (must match that plugin's `lan-bind.ts`).
const LAN_BIND_BLOCK_BEGIN: &str = "# --- remote-web-ui lan-bind block (managed - do not edit) ---";
const LAN_BIND_BLOCK_END: &str = "# --- end remote-web-ui lan-bind block ---";

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

/// The document [`assert_profile_lan_bind_port`] rewrites, for a caller that has
/// to fetch it from the machine that owns the profile first. `None` for a name
/// that is not a single path segment — the local path touches nothing then
/// either.
pub(crate) fn lan_bind_patch_path(home: &Path, profile: &str) -> Option<PathBuf> {
    profile_dir(home, profile).ok().map(|dir| patch_path(&dir))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
