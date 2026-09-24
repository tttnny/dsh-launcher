//! Launch spec module — the single owner of "how the launcher invokes DSH".
//!
//! Four call shapes drive a DSH CLI, and each carries rules that used to be
//! documented (or not) at one call site while the other three silently
//! depended on them:
//!
//! * **Instance run** ([`instance_command`]): `node NODE_RUNTIME_FLAGS bin
//!   --profile P [--port N] [--no-open]`. `--host` is NEVER passed — the
//!   profile layer owns the bind-host decision (the remote-web-ui plugin's
//!   LAN toggle). Port 0 = random.
//! * **Template boot** ([`template_boot_command`]): the ONE invocation allowed
//!   to pass `--host 127.0.0.1` — a short-lived throwaway boot whose host is
//!   the launcher's own decision.
//! * **Plugin management** ([`plugin_command`]): `dsh plugin --profile P
//!   <pnpm args>` with `CI=true` (no TTY) and the pinned pnpm prepended to
//!   PATH so the CLI's own `spawnSync("pnpm")` inherits the pin.
//! * **Terminal** ([`ensure_terminal_dsh_wrapper`] + [`terminal_env_pairs`]):
//!   a `/bin/sh` wrapper for `node_modules/.bin/dsh` plus the exported
//!   environment, so a bare `dsh` in an instance terminal behaves like the
//!   launcher's own spawns.
//!
//! The environment policy ("what env does a spawned DSH-related process get")
//! lives here too: DSH_HOME is reserved everywhere, overrides apply to
//! instances and terminals but not to `dsh plugin`, and PATH filtering is
//! terminal-only. Version-on-disk layout (npm tree vs source checkout) is
//! owned by [`is_repo_checkout`]/[`version_bin`]/[`web_app_supports_no_open`].

use crate::config::Config;
use std::path::{Path, PathBuf};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Version layout (npm tree vs source checkout)
// ---------------------------------------------------------------------------

/// Whether a version directory is a source checkout (GitHub-only tags
/// installed via clone + build) rather than an npm-installed package tree.
/// The marker is the upstream monorepo's CLI package manifest.
pub fn is_repo_checkout(version_dir: &Path) -> bool {
    version_dir
        .join("apps")
        .join("cli")
        .join("package.json")
        .exists()
}

/// The DSH CLI entry script inside an installed version tree.
pub fn version_bin(version_dir: &Path) -> PathBuf {
    if is_repo_checkout(version_dir) {
        return version_dir
            .join("apps")
            .join("cli")
            .join("lib")
            .join("bin.js");
    }
    version_dir
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js")
}

/// Checks that a version's bin.js is present and readable. On Windows, pnpm
/// hard-links store files into the version tree and a transient filesystem
/// state (antivirus scan, indexer, post-install flush) can make `exists()`
/// return false once; retry briefly before declaring the install broken.
pub fn version_bin_ready(version_dir: &Path) -> bool {
    let bin = version_bin(version_dir);
    for _ in 0..5 {
        if bin.exists() {
            if let Ok(meta) = std::fs::metadata(&bin) {
                if meta.len() > 0 {
                    return true;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    bin.exists()
}

/// Whether the installed `dsh-web-app` bundle accepts `--no-open` (added in
/// 0.1.0-rc.8). Feature-detect the flag in the bundle's startup script: the
/// flag is a string literal in `lib/startup.js`, so presence is an exact
/// signal that survives pre-release version-number formats. The bundle lives
/// under pnpm's store; we scan `node_modules/.pnpm/**/@deepseek-ai/dsh-web-app/lib/startup.js`.
pub fn web_app_supports_no_open(version_dir: &Path) -> bool {
    // Source checkouts keep the bundle at its workspace path; npm trees hoist
    // one canonical copy into the pnpm public store.
    let startup = if is_repo_checkout(version_dir) {
        version_dir
            .join("packages")
            .join("bundle")
            .join("web-app")
            .join("lib")
            .join("startup.js")
    } else {
        // The pnpm hoisted "public" store keeps one canonical copy with a
        // stable path; the hashed `.pnpm/<name>@<ver>_<hash>` layout would
        // need a scan.
        version_dir
            .join("node_modules")
            .join(".pnpm")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-web-app")
            .join("lib")
            .join("startup.js")
    };
    let Ok(raw) = std::fs::read_to_string(&startup) else {
        return false;
    };
    raw.contains("--no-open") || raw.contains("no-open")
}

// ---------------------------------------------------------------------------
// Node runtime flags
// ---------------------------------------------------------------------------

/// Node's own flags every spawned DSH process carries. They must be placed
/// *before* the CLI script path, otherwise the CLI parser sees them as DSH
/// arguments — every constructor below enforces the ordering once.
///
/// `--preserve-symlinks` keeps a `link:`-ed plugin's module URL on its profile
/// path instead of its realpath inside the plugin's checkout. DSH routes
/// kernel packages (`@deepseek-ai/*`) only for importers that live inside the
/// profiles directory, so without this flag a linked plugin falls back to
/// Node's own ancestor walk and needs a `node_modules` inside the checkout —
/// which then has to be re-pointed by hand after every DSH upgrade.
///
/// It is passed as an argument rather than via `NODE_OPTIONS` on purpose: an
/// argument applies to the DSH process alone, while `NODE_OPTIONS` would leak
/// into the `pnpm` child that `dsh plugin` spawns.
pub const NODE_RUNTIME_FLAGS: &[&str] = &["--preserve-symlinks"];

/// The base `node NODE_RUNTIME_FLAGS <bin>` invocation shared by every DSH
/// spawn; callers then add their own subcommand and flags.
fn dsh_base(version_dir: &Path) -> Result<Command, String> {
    let bin = version_bin(version_dir);
    if !version_bin_ready(version_dir) {
        return Err(format!(
            "版本安装不完整（缺少 {}），请重新安装该 DSH 版本",
            bin.display()
        ));
    }
    let mut cmd = Command::new(crate::process::node());
    crate::process::hide_console(&mut cmd);
    cmd.args(NODE_RUNTIME_FLAGS).arg(&bin);
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// Environment policy
// ---------------------------------------------------------------------------

/// Builds the effective environment for an instance: DSH_HOME (from the
/// instance's home), the launcher marker, then the user's overrides
/// (DSH_HOME is reserved and never overridden).
pub fn instance_env(cfg: &Config, instance_id: &str) -> Result<Vec<(String, String)>, String> {
    let inst = cfg
        .instances
        .iter()
        .find(|i| i.id == instance_id)
        .ok_or_else(|| "实例不存在".to_string())?;
    let home = cfg
        .homes
        .iter()
        .find(|h| h.id == inst.home_id)
        .ok_or_else(|| "DSH_HOME 不存在".to_string())?;

    let mut env: Vec<(String, String)> = Vec::new();
    env.push((
        "DSH_HOME".to_string(),
        home.path.to_string_lossy().to_string(),
    ));
    env.push(("DSH_LAUNCHER_INSTANCE".to_string(), inst.name.clone()));
    for (k, v) in &inst.env_overrides {
        if k == "DSH_HOME" {
            continue; // reserved
        }
        env.push((k.clone(), v.clone()));
    }
    // Launcher proxy applied to dsh: overrides the instance's own proxy vars.
    if cfg.settings.proxy_enabled && cfg.settings.proxy_apply_dsh {
        crate::proxy::override_env(&mut env, &cfg.settings);
    }
    Ok(env)
}

// ---------------------------------------------------------------------------
// Instance run
// ---------------------------------------------------------------------------

/// An instance's DSH process: `node FLAGS bin --profile P [--port N]
/// [--no-open]`. `web_port` is `Some(port)` only for web-app profiles (a
/// pinned port used verbatim, 0 = random free port); non-web profiles get no
/// web flags and are managed purely as processes. `--host` is deliberately
/// never passed: 127.0.0.1 is already the webserver row's default, and an
/// explicit host flag outranks every config-layer bind — @linxin666/
/// dsh-remote-web-ui resolves its LAN toggle through `desiredBindHost(lanBind,
/// startupHost)`, which returns the flag verbatim when it is
/// 127.0.0.1/0.0.0.0. Hardcoding it would make that plugin's「局域网访问」
/// toggle impossible to turn on for launcher-started instances; omitting it
/// leaves the decision where it belongs — the profile layer.
///
/// Env comes separately from [`instance_env`] so the caller controls ordering
/// relative to its own bookkeeping.
pub fn instance_command(
    version_dir: &Path,
    profile: &str,
    web_port: Option<u16>,
) -> Result<Command, String> {
    let mut cmd = dsh_base(version_dir)?;
    cmd.arg("--profile").arg(profile);
    if let Some(port) = web_port {
        cmd.arg("--port").arg(port.to_string());
        // `--no-open` was added to dsh-web-app in 0.1.0-rc.8: the launcher
        // opens the instance URL in the system browser itself (contract), so
        // the app must not open one too. Feature-detect rather than comparing
        // pre-release versions.
        if web_app_supports_no_open(version_dir) {
            cmd.arg("--no-open");
        }
    }
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// Template boot (throwaway profile generation)
// ---------------------------------------------------------------------------

/// The short-lived boot that generates the `web` profile template. The ONLY
/// DSH invocation allowed to pass `--host`: this process exists to bind one
/// port once and exit, and the host is the launcher's own decision — unlike
/// instances, where the profile layer must decide (see [`instance_command`]).
pub fn template_boot_command(
    version_dir: &Path,
    home_path: &Path,
    port: u16,
) -> Result<Command, String> {
    let mut cmd = dsh_base(version_dir)?;
    cmd.arg("--profile")
        .arg("web")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .env("DSH_HOME", home_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// Plugin management (through the instance's own CLI)
// ---------------------------------------------------------------------------

/// A `dsh plugin --profile <name> <pnpm args…>` invocation for an instance's
/// own CLI version.
///
/// Profile plugin management is a CLI-private flow: `dsh plugin` initializes
/// the profile when needed, forwards the remaining arguments to pnpm with
/// cwd = the profile directory, and then reconciles `dsh.profile.bundles`
/// against the *installed* state (a dependency whose package declares
/// `dsh.bundle.patch` joins the layer stack; one that no longer does leaves
/// it). Driving pnpm ourselves would produce a tree the CLI does not expect
/// and would leave the layer list to be guessed at, so every install and
/// removal goes through the CLI of the version that instance runs.
///
/// The CLI resolves pnpm from PATH, so the launcher's pinned pnpm is
/// prepended to PATH: the pin then also applies inside the CLI's own pnpm
/// invocation.
pub fn plugin_command(
    version_dir: &Path,
    home_path: &Path,
    profile: &str,
    pnpm_args: &[String],
    pnpm_prog: &Path,
) -> Result<Command, String> {
    let mut cmd = dsh_base(version_dir)?;
    cmd.arg("plugin")
        .arg("--profile")
        .arg(profile)
        .args(pnpm_args)
        .env("DSH_HOME", home_path)
        // The launcher can never answer an interactive prompt: pnpm aborts
        // with ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY when it needs to
        // purge a modules dir (store/virtual-store relink) without a TTY.
        // CI=true makes pnpm treat the run as non-interactive instead.
        .env("CI", "true");

    // Prepend the pinned pnpm's directory so the CLI's `spawnSync("pnpm")`
    // picks it up instead of whatever major is on the user's PATH.
    if let Some(pnpm_dir) = pnpm_prog.parent() {
        if !pnpm_dir.as_os_str().is_empty() {
            let existing = std::env::var_os("PATH").unwrap_or_default();
            let mut entries = vec![pnpm_dir.to_path_buf()];
            entries.extend(std::env::split_paths(&existing));
            match std::env::join_paths(entries) {
                Ok(joined) => {
                    cmd.env("PATH", joined);
                }
                Err(e) => {
                    crate::log_warn!("拼接 PATH 失败，沿用系统 PATH: {e}");
                }
            }
        }
    }

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// Terminal (external shell for an instance)
// ---------------------------------------------------------------------------

/// Ensures the version tree's `node_modules/.bin/dsh` is a wrapper that boots
/// the same CLI the launcher runs, with the same Node runtime flags. A
/// wrapper an earlier launcher build generated is rewritten (its flags could
/// be stale); a real pnpm-provided `dsh` is never touched.
///
/// Returns the `.bin` directory (to prepend to the terminal's PATH) when the
/// version tree exists.
pub fn ensure_terminal_dsh_wrapper(version_dir: &Path) -> Option<PathBuf> {
    let bin_dir = version_dir.join("node_modules").join(".bin");
    let dsh_bin = bin_dir.join("dsh");
    // Rewrite a wrapper an earlier launcher build generated, so its Node
    // flags cannot go stale; never touch a real pnpm-provided `dsh`.
    let wrapper_is_ours = std::fs::read_to_string(&dsh_bin)
        .map(|existing| existing.starts_with("#!/bin/sh\nexec node "))
        .unwrap_or(false);
    if !dsh_bin.exists() || wrapper_is_ours {
        let target_bin = version_bin(version_dir);
        if target_bin.exists() {
            let _ = std::fs::create_dir_all(&bin_dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // A bare `dsh` typed in this terminal boots the same
                // profiles as the launcher, so it carries the same Node
                // runtime flags (see NODE_RUNTIME_FLAGS).
                let flags = NODE_RUNTIME_FLAGS.join(" ");
                let content = format!(
                    "#!/bin/sh\nexec node {} \"{}\" \"$@\"\n",
                    flags,
                    target_bin.to_string_lossy()
                );
                if std::fs::write(&dsh_bin, content).is_ok() {
                    let _ =
                        std::fs::set_permissions(&dsh_bin, std::fs::Permissions::from_mode(0o755));
                }
            }
        }
    }
    if bin_dir.exists() {
        Some(bin_dir)
    } else {
        None
    }
}

/// The PATH directories an instance terminal prepends: the version's `.bin`
/// (via [`ensure_terminal_dsh_wrapper`]) and the launcher's managed node.
pub fn terminal_path_dirs(version_dir: Option<&Path>, data_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = version_dir.and_then(ensure_terminal_dsh_wrapper) {
        dirs.push(dir);
    }
    let managed_node_bin = data_dir.join("tools").join("node").join("bin");
    if managed_node_bin.exists() {
        dirs.push(managed_node_bin);
    }
    dirs
}

/// The environment exports for an instance terminal: DSH_HOME + the launcher
/// marker + the instance's overrides, with DSH_HOME and PATH reserved.
pub fn terminal_env_pairs(
    home_path: &Path,
    instance_name: &str,
    overrides: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut env_pairs: Vec<(String, String)> = vec![
        (
            "DSH_HOME".to_string(),
            home_path.to_string_lossy().to_string(),
        ),
        ("DSH_LAUNCHER_INSTANCE".to_string(), instance_name.to_string()),
    ];
    for (k, v) in overrides {
        if k != "DSH_HOME" && k != "PATH" {
            env_pairs.push((k.clone(), v.clone()));
        }
    }
    env_pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_app_supports_no_open_feature_detects_flag() {
        let dir = std::env::temp_dir().join(format!("dsh-launch-test-{}", uuid::Uuid::new_v4()));
        let startup_dir = dir
            .join("node_modules")
            .join(".pnpm")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-web-app")
            .join("lib");
        // No startup.js yet -> false.
        assert!(!web_app_supports_no_open(&dir));
        std::fs::create_dir_all(&startup_dir).unwrap();
        // Old bundle without the flag (<= 0.1.0-rc.7) -> false.
        std::fs::write(
            startup_dir.join("startup.js"),
            "const p = new Command().option('--host <host>').option('--port <port>')",
        )
        .unwrap();
        assert!(!web_app_supports_no_open(&dir));
        // New bundle with the flag (>= 0.1.0-rc.8) -> true.
        std::fs::write(
            startup_dir.join("startup.js"),
            "const p = new Command().option('--no-open', 'do not open the Web UI in the default browser')",
        )
        .unwrap();
        assert!(web_app_supports_no_open(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn version_bin_detects_source_checkout_layout() {
        let dir = std::env::temp_dir().join(format!("dsh-launch-test-{}", uuid::Uuid::new_v4()));
        // npm layout: node_modules/@deepseek-ai/dsh/lib/bin.js.
        assert!(version_bin(&dir).ends_with(
            std::path::Path::new("node_modules")
                .join("@deepseek-ai")
                .join("dsh")
                .join("lib")
                .join("bin.js")
        ));
        assert!(!version_bin_ready(&dir));
        // Source checkout layout (GitHub-only tags): apps/cli/lib/bin.js.
        let cli = dir.join("apps").join("cli");
        std::fs::create_dir_all(cli.join("lib")).unwrap();
        std::fs::write(cli.join("package.json"), r#"{"name":"@deepseek-ai/dsh"}"#).unwrap();
        assert_eq!(version_bin(&dir), cli.join("lib").join("bin.js"));
        assert!(!version_bin_ready(&dir));
        std::fs::write(cli.join("lib").join("bin.js"), "// bin").unwrap();
        assert!(version_bin_ready(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn web_app_no_open_detects_flag_in_source_checkout() {
        let dir = std::env::temp_dir().join(format!("dsh-launch-test-{}", uuid::Uuid::new_v4()));
        let cli = dir.join("apps").join("cli");
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::write(cli.join("package.json"), r#"{"name":"@deepseek-ai/dsh"}"#).unwrap();
        let lib = dir
            .join("packages")
            .join("bundle")
            .join("web-app")
            .join("lib");
        assert!(!web_app_supports_no_open(&dir));
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(
            lib.join("startup.js"),
            "const p = new Command().option('--no-open', 'x')",
        )
        .unwrap();
        assert!(web_app_supports_no_open(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn instance_command_orders_flags_before_script_and_profile() {
        // A minimal fake npm-layout version so dsh_base succeeds.
        let dir = std::env::temp_dir().join(format!("dsh-launch-test-{}", uuid::Uuid::new_v4()));
        let bin = dir
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("bin.js"), "// bin").unwrap();

        let cmd = instance_command(&dir, "web", Some(3099)).unwrap();
        let std_cmd = cmd.as_std();
        let argv: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        // Ordering: runtime flags, then bin.js, then DSH subcommand/flags.
        assert_eq!(argv[0], "--preserve-symlinks");
        assert!(argv[1].ends_with("bin.js"));
        assert_eq!(argv[2], "--profile");
        assert_eq!(argv[3], "web");
        assert_eq!(argv[4], "--port");
        assert_eq!(argv[5], "3099");
        // The instance run never passes --host (profile layer owns the bind).
        assert!(!argv.iter().any(|a| a == "--host"));

        // Non-web profile: no --port, no --no-open.
        let plain = instance_command(&dir, "bot", None).unwrap();
        let plain_argv: Vec<String> = plain
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(!plain_argv.iter().any(|a| a.starts_with("--port")));
        assert!(!plain_argv.iter().any(|a| a == "--no-open"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn template_boot_passes_host_and_pinned_port() {
        let dir = std::env::temp_dir().join(format!("dsh-launch-test-{}", uuid::Uuid::new_v4()));
        let bin = dir
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("bin.js"), "// bin").unwrap();

        let cmd = template_boot_command(&dir, Path::new("/tmp/home"), 20001).unwrap();
        let argv: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let host = argv.iter().position(|a| a == "--host").unwrap();
        assert_eq!(argv[host + 1], "127.0.0.1");
        let port = argv.iter().position(|a| a == "--port").unwrap();
        assert_eq!(argv[port + 1], "20001");
        // argv = [flags, bin.js, --profile, web, --host, 127.0.0.1, --port, N]
        assert_eq!(argv[2], "--profile");
        assert_eq!(argv[3], "web");
        std::fs::remove_dir_all(&dir).ok();
    }
}
