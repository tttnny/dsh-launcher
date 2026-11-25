use std::path::{Path, PathBuf};

pub(crate) fn profiles_dir(home: &Path) -> PathBuf {
    home.join("profiles")
}

pub(crate) fn is_single_path_segment(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}

pub(crate) fn profile_dir(home: &Path, profile: &str) -> Result<PathBuf, String> {
    if !is_single_path_segment(profile) {
        return Err(format!("Profile 名称不合法: {profile}"));
    }
    Ok(profiles_dir(home).join(profile))
}

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

fn patch_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("cordis.patch.yml")
}

/// Marker lines the `@linxin666/dsh-remote-web-ui` plugin wraps its managed
/// lan-bind block in (must match that plugin's `lan-bind.ts`).
const LAN_BIND_BLOCK_BEGIN: &str = "# --- remote-web-ui lan-bind block (managed - do not edit) ---";
const LAN_BIND_BLOCK_END: &str = "# --- end remote-web-ui lan-bind block ---";

pub(crate) fn scrub_port_pin(raw: &str) -> String {
    rewrite_row_in_id_block(raw, "webserver", "port", "0").0
}

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
        assert!(
            out.contains("port: 1"),
            "block outside managed area kept: {out}"
        );
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
