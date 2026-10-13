//! Profile module — owns the profile layout under a DSH_HOME and the profile
//! patch document (`cordis.patch.yml`).
//!
//! A profile is one directory under `<DSH_HOME>/profiles/<name>`. Its patch
//! document's plugin entries are written by the CLI (insert rows when a plugin
//! is installed) and the `@linxin666/dsh-remote-web-ui` plugin manages a marked
//! lan-bind block that pins the web port. The launcher never writes plugin
//! state: patch layers and their entries belong to the running DSH (ADR-0001),
//! and the plugin page talks to that process over HTTP instead. What the
//! launcher still owns here are the two *read* facts a start needs — the port
//! pin that must land before launch ([`scrub_port_pin`] /
//! [`assert_profile_lan_bind_port`]) and whether the manifest declares a
//! `link:` dependency ([`preserve_symlinks_for`]).
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

/// Node's `--preserve-symlinks` decision for an instance launched with this
/// HOME and profile.
///
/// The launcher derives it from the profile manifest instead of asking the
/// user: the flag exists for exactly one situation (a profile that declares a
/// `link:` dependency), and it is actively harmful everywhere else — see
/// [`crate::launch::node_runtime_flags`]. A setting the user must keep in sync
/// with a fact the launcher can read is a setting that will be wrong.
///
/// Unknown HOME/profile reads as `false`: an unreadable manifest means the
/// profile has probably not been installed yet, and the flag's failure mode
/// (every DSH settings write rejected) is much worse than the one it prevents.
pub(crate) fn preserve_symlinks_for(home: &Path, profile: &str) -> bool {
    if profile.is_empty() {
        return false;
    }
    profile_dir(home, profile)
        .map(|dir| declares_link_dependency(&dir))
        .unwrap_or(false)
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

    /// An instance derives its flag from the profile it launches with, so the
    /// HOME + profile name pair is the whole input, and anything unresolvable
    /// stays off.
    #[test]
    fn preserve_symlinks_for_resolves_the_instance_profile() {
        let home = std::env::temp_dir().join(format!("dsh-psl-home-{}", uuid::Uuid::new_v4()));
        let dir = home.join("profiles").join("linked");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"my-plugin":"link:../my-plugin"}}"#,
        )
        .unwrap();
        assert!(preserve_symlinks_for(&home, "linked"));

        // No profile name (an instance that has never been assigned one) and a
        // profile that does not exist both read as "off".
        assert!(!preserve_symlinks_for(&home, ""));
        assert!(!preserve_symlinks_for(&home, "missing"));

        // A name that would escape the HOME's profiles directory is refused
        // rather than resolved.
        assert!(!preserve_symlinks_for(&home, "../../etc"));

        std::fs::remove_dir_all(&home).ok();
    }
}
