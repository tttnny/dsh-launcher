use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

use crate::AppState;

#[derive(Clone, Debug, Serialize)]
pub struct ToolStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeStatus {
    pub node: ToolStatus,
    pub pnpm: ToolStatus,
    /// Which Node version managers are present (`nvm`, `fnm`, `brew`), so the
    /// setup page can hand out a command that fits the machine instead of one
    /// fixed recipe. Order is by preference: a manager the user already has is
    /// always the right thing to extend.
    pub managers: Vec<String>,
    /// True when node is present but older than the version DSH is developed
    /// against. Advisory: the launcher does not block on it, because DSH itself
    /// declares no engine requirement.
    pub node_below_recommended: bool,
    /// The numbers the setup page has to state, carried from the constants that
    /// enforce them so the UI copy cannot drift out of sync with what the
    /// backend actually accepts.
    pub requirements: ToolchainRequirements,
}

/// What the launcher needs from the user's toolchain. Sent to the frontend so
/// text like "需要 pnpm 11 或更高" renders from the same value the resolver
/// checks, instead of a second hardcoded literal.
#[derive(Clone, Debug, Serialize)]
pub struct ToolchainRequirements {
    pub min_pnpm_major: u32,
    pub recommended_node_major: u64,
    pub pnpm_install_command: String,
    /// The Node install command to suggest for *this* machine.
    ///
    /// Composed here rather than in the UI because the choice depends on which
    /// manager was detected and in what preference order — knowledge that lives
    /// in [`detected_managers`]. A second copy of that cascade in the frontend
    /// could disagree with the detection it is describing.
    pub node_install_command: String,
}

impl ToolchainRequirements {
    fn current(managers: &[String]) -> Self {
        Self {
            min_pnpm_major: crate::toolchain::MIN_PNPM_MAJOR,
            recommended_node_major: RECOMMENDED_NODE_MAJOR,
            pnpm_install_command: crate::toolchain::PNPM_INSTALL_HINT.to_string(),
            node_install_command: node_install_command(managers),
        }
    }
}

/// The Node install command for a machine that has these managers: extend the
/// one the user already uses, so they end up with one Node in the place they
/// expect. With no manager at all, the recipe installs nvm first (it is the
/// least invasive option — no admin rights, no system-wide change).
fn node_install_command(managers: &[String]) -> String {
    let has = |m: &str| managers.iter().any(|x| x == m);
    if has("nvm") {
        return "nvm install --lts".to_string();
    }
    if has("fnm") {
        return "fnm install --lts".to_string();
    }
    if has("brew") {
        return "brew install node".to_string();
    }
    "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash\nnvm install --lts"
        .to_string()
}

/// The Node major the launcher recommends. README states it as a development
/// prerequisite; DSH's own `engines` field is empty, so this is a warning
/// threshold, never a hard gate.
pub(crate) const RECOMMENDED_NODE_MAJOR: u64 = 20;

/// Every path a program of this name may live at, in resolution order: the
/// inherited PATH first (so an explicitly configured toolchain wins), then the
/// known tool directories. Shared by the UI probe and by
/// [`crate::toolchain`]'s resolver, so "what the setup page reports" and "what
/// the launcher actually executes" can never disagree.
pub(crate) fn candidate_paths(program: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    for dir in tool_fallback_bins() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }

    let mut out = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let exe = if dir.as_os_str().is_empty() {
            PathBuf::from(program)
        } else {
            dir.join(program)
        };
        if !out.contains(&exe) {
            out.push(exe);
        }
    }
    out
}

async fn probe(program: &str) -> ToolStatus {
    // GUI-launched apps inherit a minimal PATH with no shell rc, so a plain
    // Command::new(program) misses fnm/nvm/Homebrew/user-installed tools.
    // candidate_paths walks PATH and then the known tool directories
    // (fnm/nvm layouts, PNPM_HOME, Homebrew, …).
    for exe in candidate_paths(program) {
        if !exe.is_file() {
            continue;
        }
        let mut cmd = tokio::process::Command::new(&exe);
        crate::process::hide_console(&mut cmd);
        // A neutral cwd keeps the reported version honest (see PROBE_CWD).
        cmd.current_dir(PROBE_CWD);
        if let Ok(out) = cmd.arg("--version").output().await {
            if out.status.success() {
                let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return ToolStatus {
                    installed: true,
                    version: Some(version),
                    path: Some(exe.to_string_lossy().into_owned()),
                };
            }
        }
    }
    ToolStatus {
        installed: false,
        version: None,
        path: None,
    }
}

/// Probes node and pnpm and records the result in the config, so every later
/// spawn can use an absolute path instead of re-resolving a name against
/// whatever PATH the app inherited. Returns the binding that was written.
///
/// Freshness is checked per tool: a binding whose path still exists on disk is
/// kept, so a startup probe does not re-run `--version` for a toolchain that
/// has not moved. A binding whose path vanished (an nvm version the user
/// switched away from, a removed brew formula) is re-probed and updated — that
/// is the whole point of persisting a *resolution* rather than a preference.
pub async fn refresh_toolchain_binding(
    state: &State<'_, AppState>,
) -> crate::config::ToolchainBinding {
    let current = state.config.lock().unwrap().settings.toolchain.clone();
    // Both freshness checks run before either field is moved out.
    let (node_fresh, pnpm_fresh) = (current.node_is_fresh(), current.pnpm_is_fresh());

    // node: keep the recorded path when it still exists.
    let node = if node_fresh {
        current.node
    } else {
        bound_from(probe("node").await)
    };
    // pnpm: reuse the resolver, so the bound binary is exactly the one the
    // install flows will drive (and its version already meets the floor).
    let pnpm = if pnpm_fresh {
        current.pnpm
    } else {
        let mut notes = Vec::new();
        match crate::toolchain::resolve_pnpm(&mut |s| notes.push(s)).await {
            Some((path, version)) => Some(crate::config::BoundTool { path, version }),
            None => {
                for note in notes {
                    crate::log_warn!("{note}");
                }
                None
            }
        }
    };
    let binding = crate::config::ToolchainBinding { node, pnpm };

    {
        let mut cfg = state.config.lock().unwrap();
        if !binding_matches(&cfg.settings.toolchain, &binding) {
            cfg.settings.toolchain = binding.clone();
            if let Err(e) = crate::config::save_config(&state.config_path, &cfg) {
                crate::log_warn!("保存工具链绑定失败: {e}");
            }
        }
    }
    binding
}

fn bound_from(status: ToolStatus) -> Option<crate::config::BoundTool> {
    match (status.path, status.version) {
        (Some(path), Some(version)) => Some(crate::config::BoundTool {
            path: PathBuf::from(path),
            version,
        }),
        _ => None,
    }
}

/// Whether two bindings describe the same tools, so an unchanged probe does not
/// rewrite the config file on every startup.
fn binding_matches(
    a: &crate::config::ToolchainBinding,
    b: &crate::config::ToolchainBinding,
) -> bool {
    fn same(
        x: &Option<crate::config::BoundTool>,
        y: &Option<crate::config::BoundTool>,
    ) -> bool {
        match (x, y) {
            (Some(x), Some(y)) => x.path == y.path && x.version == y.version,
            (None, None) => true,
            _ => false,
        }
    }
    same(&a.node, &b.node) && same(&a.pnpm, &b.pnpm)
}

/// The node executable every DSH spawn should use, for callers that hold a
/// config snapshot. Falls back to the bare name only if the binding is absent
/// (a config written before the binding existed), in which case PATH resolution
/// is still better than refusing to launch.
fn node_for_spawn(cfg: &crate::config::Config) -> PathBuf {
    match &cfg.settings.toolchain.node {
        Some(bound) if bound.path.is_file() => bound.path.clone(),
        _ => PathBuf::from(crate::process::node()),
    }
}

/// Whether a spawn has a usable node. Node is required to run DSH at all, so
/// this is the one thing that must be present; pnpm only matters for installs
/// and plugin management (both of which have their own actionable error).
pub fn node_missing_error() -> String {
    "未找到可用的 Node.js（需要 20 或更高版本）。\n\
     请在终端安装（如 nvm install --lts）后，回到「设置 → 环境」点重新检测。"
        .to_string()
}

/// Probes node and returns its absolute path, refreshing a stale binding first.
/// Used by spawn paths so a toolchain installed while the app was running is
/// picked up without a restart.
pub async fn node_for_spawn_checked(
    state: &State<'_, AppState>,
) -> Result<PathBuf, String> {
    let fresh = {
        let cfg = state.config.lock().unwrap();
        cfg.settings.toolchain.node_is_fresh()
    };
    if fresh {
        let cfg = state.config.lock().unwrap();
        return Ok(node_for_spawn(&cfg));
    }
    let binding = refresh_toolchain_binding(state).await;
    match binding.node {
        Some(bound) => Ok(bound.path),
        None => Err(node_missing_error()),
    }
}

#[tauri::command]
pub async fn get_runtime_status(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    // One probe path for both the UI and the binding: the numbers the setup
    // page shows are the ones the launcher will actually spawn.
    let binding = refresh_toolchain_binding(&state).await;
    let node = status_from(&binding.node);
    let managers = detected_managers();
    Ok(RuntimeStatus {
        node_below_recommended: node_is_below_recommended(&node),
        node,
        pnpm: status_from(&binding.pnpm),
        requirements: ToolchainRequirements::current(&managers),
        managers,
    })
}

/// Whether the probed node reports a major older than
/// [`RECOMMENDED_NODE_MAJOR`]. An unparsable version is not flagged: the
/// version string of a working node is not worth a warning.
fn node_is_below_recommended(node: &ToolStatus) -> bool {
    let Some(version) = node.version.as_deref() else {
        return false;
    };
    match node_major(version) {
        Some(major) => major < RECOMMENDED_NODE_MAJOR,
        None => false,
    }
}

/// Parses the major out of `node --version` output ("v22.14.0" or "22.14.0").
pub(crate) fn node_major(version_output: &str) -> Option<u64> {
    version_output
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse::<u64>()
        .ok()
}

/// Which Node version managers exist on this machine, most preferred first.
/// Only managers whose directory layout the scanner already understands are
/// reported, so the setup page never suggests a tool the launcher cannot then
/// find. Pure filesystem checks — no subprocess.
fn detected_managers() -> Vec<String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut found = Vec::new();
    if home.join(".nvm").is_dir() {
        found.push("nvm".to_string());
    }
    if !fnm_bin_dirs(&home).is_empty() || home.join(".fnm").is_dir() {
        found.push("fnm".to_string());
    }
    if Path::new("/opt/homebrew/bin/brew").is_file()
        || Path::new("/usr/local/bin/brew").is_file()
    {
        found.push("brew".to_string());
    }
    found
}

fn status_from(tool: &Option<crate::config::BoundTool>) -> ToolStatus {
    match tool {
        Some(bound) => ToolStatus {
            installed: true,
            version: Some(bound.version.clone()),
            path: Some(bound.path.to_string_lossy().into_owned()),
        },
        None => ToolStatus {
            installed: false,
            version: None,
            path: None,
        },
    }
}

/// fnm-managed Node bin directories, most specific first:
///
/// 1. the `default` alias (`<fnm-root>/aliases/default/bin`)
/// 2. `<fnm-root>/current/bin`
/// 3. every `<fnm-root>/node-versions/<v>/installation/bin`, newest first.
///
/// Both the modern XDG layout (`~/.local/share/fnm`) and the legacy layout
/// (`~/.fnm`) are scanned.
fn fnm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in [
        home.join(".fnm"),
        home.join(".local").join("share").join("fnm"),
    ] {
        let alias = root.join("aliases").join("default").join("bin");
        if alias.is_dir() && !dirs.contains(&alias) {
            dirs.push(alias);
        }
        let current = root.join("current").join("bin");
        if current.is_dir() && !dirs.contains(&current) {
            dirs.push(current);
        }
        let versions = root.join("node-versions");
        if let Ok(rd) = std::fs::read_dir(&versions) {
            let mut found: Vec<(semver::Version, PathBuf)> = rd
                .filter_map(|e| {
                    let e = e.ok()?;
                    let name = e.file_name().to_string_lossy().into_owned();
                    let ver = name
                        .strip_prefix('v')
                        .and_then(|s| semver::Version::parse(s).ok())?;
                    Some((ver, e.path()))
                })
                .collect();
            found.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
            for (_, p) in found {
                let bin = p.join("installation").join("bin");
                if bin.is_dir() && !dirs.contains(&bin) {
                    dirs.push(bin);
                }
            }
        }
    }
    dirs
}

/// nvm-managed Node bin directories, most specific first:
///
/// 1. the version `~/.nvm/alias/default` points at
/// 2. every `<root>/versions/node/<v>/bin`, newest first.
///
/// Only `~/.nvm` is scanned: that is where the installer puts it, and the
/// location is not relocatable without `$NVM_DIR` being exported (which a
/// Finder-launched app does not inherit).
fn nvm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm");
    if !root.is_dir() {
        return Vec::new();
    }
    let mut dirs: Vec<PathBuf> = Vec::new();

    // `nvm alias default v20.11.0` writes a bare version. Indirect aliases
    // (`lts/*`, `node`) resolve to another alias and are not usable here, so
    // anything that is not a bare version is ignored.
    if let Ok(raw) = std::fs::read_to_string(root.join("alias").join("default")) {
        let alias = raw.trim();
        if semver::Version::parse(alias.strip_prefix('v').unwrap_or(alias)).is_ok() {
            let bin = root.join("versions").join("node").join(alias).join("bin");
            if bin.is_dir() {
                dirs.push(bin);
            }
        }
    }

    if let Ok(rd) = std::fs::read_dir(root.join("versions").join("node")) {
        let mut found: Vec<(semver::Version, PathBuf)> = rd
            .filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name().to_string_lossy().into_owned();
                let ver = name
                    .strip_prefix('v')
                    .and_then(|s| semver::Version::parse(s).ok())?;
                Some((ver, e.path()))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
        for (_, p) in found {
            let bin = p.join("bin");
            if bin.is_dir() && !dirs.contains(&bin) {
                dirs.push(bin);
            }
        }
    }
    dirs
}

/// Directories holding a standalone `pnpm` binary. The official installer
/// puts it in `PNPM_HOME/bin` on macOS (`~/Library/pnpm/bin`), while the v10
/// era linked it straight into `PNPM_HOME` — both layouts are scanned, and an
/// explicit `$PNPM_HOME` wins because that is where pnpm itself was told to
/// live.
fn pnpm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(explicit) = std::env::var_os("PNPM_HOME").map(PathBuf::from) {
        if !explicit.as_os_str().is_empty() {
            roots.push(explicit);
        }
    }
    roots.push(home.join("Library").join("pnpm"));
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        if !xdg.as_os_str().is_empty() {
            roots.push(xdg.join("pnpm"));
        }
    }
    roots.push(home.join(".local").join("share").join("pnpm"));

    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in roots {
        // `/bin` first: a root that has both layouts should resolve the
        // installer-managed one.
        for candidate in [root.join("bin"), root.clone()] {
            if candidate.is_dir() && !dirs.contains(&candidate) {
                dirs.push(candidate);
            }
        }
    }
    dirs
}

/// The cwd every version probe runs in. `--version` must not depend on the
/// app's own working directory: pnpm resolves a `packageManager` field from the
/// cwd and reports *that* version (and a corepack shim would download it), so
/// probing from a neutral directory is what makes the reported version
/// truthful. Shared with [`crate::toolchain`]'s resolver, which runs the same
/// probe.
pub(crate) const PROBE_CWD: &str = "/";

/// Known directories where user-level tools (node/pnpm via nvm/fnm,
/// standalone pnpm, bun, Homebrew) may live — used as a probe fallback when
/// PATH alone would miss them.
fn tool_fallback_bins() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut dirs = fnm_bin_dirs(&home);
    for d in nvm_bin_dirs(&home) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    for d in pnpm_bin_dirs(&home) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    for d in [
        home.join(".bun").join("bin"),
        home.join(".cargo").join("bin"),
        home.join(".local").join("bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ] {
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// Ensures the standard macOS tool paths — nvm/fnm-managed Node, standalone
/// pnpm, Homebrew, and user bin directories — are on PATH even when launched
/// from Finder without an interactive shell, so node/pnpm/git resolve for
/// spawned children. Reuses the probe's directory scan so the two can never
/// disagree about where a tool lives.
pub fn ensure_macos_paths() {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut parts: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();

    // The same candidates the probe walks (nvm/fnm versions, PNPM_HOME, …),
    // so anything the UI reports as found is also reachable by our children.
    let mut extra: Vec<PathBuf> = tool_fallback_bins();
    for d in [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        extra.push(PathBuf::from(d));
    }
    if !home.as_os_str().is_empty() {
        extra.push(home.join(".local").join("bin"));
        extra.push(home.join(".cargo").join("bin"));
    }

    for p in extra {
        if p.is_dir() && !parts.contains(&p) {
            parts.push(p);
        }
    }
    if let Ok(joined) = std::env::join_paths(parts) {
        std::env::set_var("PATH", joined);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree(home: &Path) {
        std::fs::create_dir_all(
            home.join(".local/share/fnm/node-versions/v20.11.0/installation/bin"),
        )
        .unwrap();
        std::fs::create_dir_all(
            home.join(".local/share/fnm/node-versions/v24.16.0/installation/bin"),
        )
        .unwrap();
    }

    #[test]
    fn fnm_dirs_prefer_default_alias_then_newest_version() {
        let home = std::env::temp_dir().join(format!("fnm-test-{}", uuid::Uuid::new_v4()));
        make_tree(&home);
        // No alias: newest installed version's bin wins.
        let dirs = fnm_bin_dirs(&home);
        assert!(!dirs.is_empty());
        assert!(dirs[0].ends_with("node-versions/v24.16.0/installation/bin"));
        // With a default alias, it ranks first.
        let alias = home.join(".local/share/fnm/aliases/default/bin");
        std::fs::create_dir_all(&alias).unwrap();
        let dirs = fnm_bin_dirs(&home);
        assert_eq!(dirs[0], alias);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn fnm_dirs_handles_legacy_root() {
        let home = std::env::temp_dir().join(format!("fnm-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".fnm/current/bin")).unwrap();
        let dirs = fnm_bin_dirs(&home);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with(".fnm/current/bin"));
        std::fs::remove_dir_all(&home).ok();
    }

    fn tmp_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn nvm_dirs_prefer_default_alias_then_newest_version() {
        let home = tmp_home("nvm-test");
        std::fs::create_dir_all(home.join(".nvm/versions/node/v20.11.0/bin")).unwrap();
        std::fs::create_dir_all(home.join(".nvm/versions/node/v24.16.0/bin")).unwrap();

        // No alias: newest installed version wins.
        let dirs = nvm_bin_dirs(&home);
        assert_eq!(dirs[0], home.join(".nvm/versions/node/v24.16.0/bin"));

        // `nvm alias default v20.11.0` writes the version into the alias file.
        std::fs::create_dir_all(home.join(".nvm/alias")).unwrap();
        std::fs::write(home.join(".nvm/alias/default"), "v20.11.0\n").unwrap();
        let dirs = nvm_bin_dirs(&home);
        assert_eq!(dirs[0], home.join(".nvm/versions/node/v20.11.0/bin"));

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn nvm_dirs_ignore_indirect_alias_and_missing_root() {
        let home = tmp_home("nvm-test");
        // `lts/*` and `node` are alias-of-alias indirections, not versions.
        std::fs::create_dir_all(home.join(".nvm/alias")).unwrap();
        std::fs::write(home.join(".nvm/alias/default"), "lts/*\n").unwrap();
        assert!(nvm_bin_dirs(&home).is_empty());

        // Absent ~/.nvm contributes nothing.
        let bare = tmp_home("nvm-bare");
        assert!(nvm_bin_dirs(&bare).is_empty());
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&bare).ok();
    }

    #[test]
    fn pnpm_dirs_cover_bin_subdirectory_layout() {
        let home = tmp_home("pnpm-test");
        // The official `get.pnpm.io/install.sh` layout puts the binary in
        // PNPM_HOME/bin on macOS, not directly in PNPM_HOME.
        std::fs::create_dir_all(home.join("Library/pnpm/bin")).unwrap();
        let dirs = pnpm_bin_dirs(&home);
        assert!(
            dirs.contains(&home.join("Library/pnpm/bin")),
            "expected the official PNPM_HOME/bin layout, got {dirs:?}"
        );
        // The v10-era layout linked straight into PNPM_HOME; keep it too.
        std::fs::create_dir_all(home.join("Library/pnpm")).ok();
        assert!(pnpm_bin_dirs(&home).contains(&home.join("Library/pnpm")));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn probe_runs_with_a_neutral_cwd() {
        // pnpm's `--version` is cwd-dependent: a corepack shim (and the pnpm 12
        // binary itself) honour a `packageManager` field found in the working
        // directory, so the same binary reports different versions per cwd.
        // Probing must therefore not inherit the app's cwd.
        assert_eq!(PROBE_CWD, "/");
    }

    #[test]
    fn node_major_parses_version_output() {
        assert_eq!(node_major("v22.14.0"), Some(22));
        assert_eq!(node_major("22.14.0\n"), Some(22));
        assert_eq!(node_major("  v20.11.1  "), Some(20));
        assert_eq!(node_major("not-a-version"), None);
        assert_eq!(node_major(""), None);
    }

    #[test]
    fn node_below_recommended_flags_only_old_parsable_versions() {
        let with = |v: Option<&str>| ToolStatus {
            installed: v.is_some(),
            version: v.map(|s| s.to_string()),
            path: None,
        };
        // Other majors are supported; 20 is the recommendation floor.
        assert!(node_is_below_recommended(&with(Some("v18.19.0"))));
        assert!(!node_is_below_recommended(&with(Some("v20.0.0"))));
        assert!(!node_is_below_recommended(&with(Some("v24.16.0"))));
        // A missing or unparsable version must not produce a warning.
        assert!(!node_is_below_recommended(&with(None)));
        assert!(!node_is_below_recommended(&with(Some("weird"))));
    }

    #[test]
    fn node_install_command_extends_whatever_manager_exists() {
        let m = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // An existing manager is always the right thing to extend, and the
        // preference order is the detection order.
        assert_eq!(node_install_command(&m(&["nvm"])), "nvm install --lts");
        assert_eq!(node_install_command(&m(&["fnm"])), "fnm install --lts");
        assert_eq!(node_install_command(&m(&["brew"])), "brew install node");
        assert_eq!(node_install_command(&m(&["nvm", "brew"])), "nvm install --lts");
        assert_eq!(node_install_command(&m(&["fnm", "brew"])), "fnm install --lts");

        // Nothing to extend: install a manager first, then Node.
        let bare = node_install_command(&m(&[]));
        assert!(bare.contains("nvm-sh/nvm"), "should install nvm first: {bare}");
        assert!(bare.contains("nvm install --lts"));
    }
}
