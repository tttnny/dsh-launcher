//! Launch spec module — the single owner of "how the launcher invokes DSH".
//!
//! Three call shapes drive a DSH CLI, and each carries rules that used to be
//! documented (or not) at one call site while the other two silently
//! depended on them:
//!
//! * **Instance run** ([`instance_command`]): `node [flags] bin
//!   --profile P [--port N] [--no-open]`. `--host` is NEVER passed — the
//!   profile layer owns the bind-host decision (the remote-web-ui plugin's
//!   LAN toggle). Port 0 = random.
//! * **Template boot** ([`template_boot_command`]): the ONE invocation allowed
//!   to pass `--host 127.0.0.1` — a short-lived throwaway boot whose host is
//!   the launcher's own decision.
//! * **Terminal** ([`ensure_terminal_dsh_wrapper`] + [`terminal_env_pairs`]):
//!   a `/bin/sh` wrapper for `node_modules/.bin/dsh` plus the exported
//!   environment, so a bare `dsh` in an instance terminal behaves like the
//!   launcher's own spawns.
//!
//! The environment policy ("what env does a spawned DSH-related process get")
//! lives here too: DSH_HOME is reserved everywhere, overrides apply to
//! instances and terminals, and PATH filtering is terminal-only.
//! Version-on-disk layout (npm tree vs source checkout) is owned by
//! [`is_repo_checkout`]/[`version_bin`]/[`web_app_supports_no_open`].

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
/// signal that survives pre-release version-number formats.
pub fn web_app_supports_no_open(version_dir: &Path) -> bool {
    match std::fs::read_to_string(web_app_startup_script(version_dir)) {
        Ok(raw) => supports_no_open(&raw),
        Err(_) => false,
    }
}

/// Where that bundle's startup script lives inside a version tree. Separated
/// from the reading so a caller that has the file's text from somewhere else —
/// the machine that owns the version tree — asks the same question of it.
pub fn web_app_startup_script(version_dir: &Path) -> PathBuf {
    // Source checkouts keep the bundle at its workspace path; npm trees hoist
    // one canonical copy into the pnpm public store.
    if is_repo_checkout(version_dir) {
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
    }
}

/// Whether one bundle's startup script accepts `--no-open` — the flag the
/// launcher needs because it opens the instance URL in the system browser
/// itself, so DSH must not open one too.
pub fn supports_no_open(startup_script: &str) -> bool {
    startup_script.contains("--no-open") || startup_script.contains("no-open")
}

/// The base `node <bin>` invocation shared by every DSH spawn; callers then
/// add their own subcommand and flags. No Node runtime flag rides ahead of
/// `bin.js`: the launcher once passed `--preserve-symlinks` for profiles that
/// declare a `link:` dependency, and that flag makes Node reach one package
/// through several distinct paths. DSH keys module-level state on identity
/// (the tool scheduler is a module-level `Symbol`, `@deepseek-ai/dsh-app-boot`
/// a module-level WeakMap), so a duplicated instance breaks the process:
/// every tool call fails with `Cannot read properties of undefined (reading
/// 'prepare')`. Kernel-package routing covers `link:` plugins without it.
///
/// `node` is the resolved absolute path from the launcher's toolchain binding,
/// not the bare name: a Finder-launched app inherits a minimal PATH, so
/// resolving the interpreter at spawn time made every instance depend on the
/// environment the app happened to start with (and on pnpm/nvm shims that
/// re-resolve per cwd). Passing it in keeps that decision in one place while
/// leaving argv ordering — this module's real contract — here.
fn dsh_base(node: &Path, version_dir: &Path) -> Result<Command, String> {
    let bin = version_bin(version_dir);
    if !version_bin_ready(version_dir) {
        return Err(format!(
            "版本安装不完整（缺少 {}），请重新安装该 DSH 版本",
            bin.display()
        ));
    }
    let mut cmd = Command::new(node);
    crate::process::hide_console(&mut cmd);
    cmd.arg(&bin);
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// Environment policy
// ---------------------------------------------------------------------------

/// Builds the effective environment for an instance: DSH_HOME (from the
/// instance's home), the launcher marker, then the user's overrides
/// (DSH_HOME is reserved and never overridden).
pub fn instance_env(cfg: &Config, instance_id: &str) -> Result<Vec<(String, String)>, String> {
    instance_env_for(cfg, instance_id, &crate::config::Host::local())
}

/// The same policy, composed for the machine the instance runs on.
///
/// 本机's environment is what it always was, the launcher's proxy included.
/// Another machine's never carries the launcher's proxy settings: those describe
/// *this* Mac's network exit, and what that host goes out through is decided by
/// its own record's environment variables (ADR-0006 §3 / issue #9
/// 「远程出口由主机记录决定」). Those ride last, the position the proxy holds on
/// 本机 — the general network setting of the machine outranks one instance's own
/// choice of variable.
pub fn instance_env_for(
    cfg: &Config,
    instance_id: &str,
    host: &crate::config::Host,
) -> Result<Vec<(String, String)>, String> {
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
    if host.builtin {
        // Launcher proxy applied to dsh: overrides the instance's own proxy vars.
        if cfg.settings.proxy_enabled && cfg.settings.proxy_apply_dsh {
            crate::proxy::override_env(&mut env, &cfg.settings);
        }
    } else {
        for (k, v) in &host.extra_env {
            if k == "DSH_HOME" {
                continue; // reserved here too
            }
            env.push((k.clone(), v.clone()));
        }
    }
    Ok(env)
}

// ---------------------------------------------------------------------------
// Instance run
// ---------------------------------------------------------------------------

/// An instance's DSH process: `node bin --profile P [--port N]
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
    node: &Path,
    version_dir: &Path,
    profile: &str,
    web_port: Option<u16>,
) -> Result<Command, String> {
    let mut cmd = dsh_base(node, version_dir)?;
    let no_open = web_port.is_some() && web_app_supports_no_open(version_dir);
    cmd.args(instance_flags(profile, web_port, no_open));
    Ok(cmd)
}

/// The flags one instance run carries, in order, after the `bin.js`:
/// `--profile P [--port N] [--no-open]`.
///
/// `no_open` is the caller's answer because on another machine it comes from
/// that machine's own copy of the bundle ([`supports_no_open`] over the file it
/// reads there). The rest — that `--host` is never among them, and that port 0
/// means a random free port — holds wherever the process ends up.
pub fn instance_flags(profile: &str, web_port: Option<u16>, no_open: bool) -> Vec<String> {
    let mut args = vec!["--profile".to_string(), profile.to_string()];
    if let Some(port) = web_port {
        args.push("--port".to_string());
        args.push(port.to_string());
        if no_open {
            args.push("--no-open".to_string());
        }
    }
    args
}

/// The whole command line of one instance run: the resolved interpreter, the
/// version's `bin.js`, then [`instance_flags`].
///
/// [`instance_command`] is this plus a `tokio::process::Command`, which a
/// remote launcher cannot use: the process it starts is started by that
/// machine's own shell, detached from the ssh session. Handing the same words
/// to the transport is what keeps the flag policy from drifting between the two
/// machines.
pub fn instance_argv(
    node: &Path,
    version_dir: &Path,
    profile: &str,
    web_port: Option<u16>,
    no_open: bool,
) -> Vec<String> {
    let mut argv = vec![
        node.to_string_lossy().to_string(),
        version_bin(version_dir).to_string_lossy().to_string(),
    ];
    argv.extend(instance_flags(profile, web_port, no_open));
    argv
}

// ---------------------------------------------------------------------------
// Template boot (throwaway profile generation)
// ---------------------------------------------------------------------------

/// The short-lived boot that generates the `web` profile template. The ONLY
/// DSH invocation allowed to pass `--host`: this process exists to bind one
/// port once and exit, and the host is the launcher's own decision — unlike
/// instances, where the profile layer must decide (see [`instance_command`]).
pub fn template_boot_command(
    node: &Path,
    version_dir: &Path,
    home_path: &Path,
    port: u16,
) -> Result<Command, String> {
    let mut cmd = dsh_base(node, version_dir)?;
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
// Terminal (external shell for an instance)
// ---------------------------------------------------------------------------

/// Ensures the version tree's `node_modules/.bin/dsh` is a wrapper that boots
/// the same CLI the launcher runs. A wrapper an earlier launcher build wrote
/// is rewritten (it carried `--preserve-symlinks`); a real pnpm-provided
/// `dsh` is never touched.
///
/// Returns the `.bin` directory (to prepend to the terminal's PATH) when the
/// version tree exists.
pub fn ensure_terminal_dsh_wrapper(version_dir: &Path) -> Option<PathBuf> {
    let bin_dir = version_bin_dir(version_dir);
    let dsh_bin = bin_dir.join("dsh");

    // The wrapper this build wants, or `None` when the version tree has no
    // bin.js to point at (a partial install has nothing to write).
    let desired = wrapper_script(version_dir);
    if let Some(content) = desired {
        let existing = std::fs::read_to_string(&dsh_bin).ok();
        // Never touch a real pnpm-provided `dsh`. Our own wrapper is rewritten
        // whenever its content differs, so a wrapper an earlier build wrote
        // cannot keep the removed flag alive in instance terminals.
        let wrapper_is_ours = existing
            .as_deref()
            .is_some_and(|raw| raw.starts_with("#!/bin/sh\nexec node "));
        if (!dsh_bin.exists() || wrapper_is_ours) && existing.as_deref() != Some(content.as_str()) {
            let _ = std::fs::create_dir_all(&bin_dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // A bare `dsh` typed in this terminal boots the same
                // profiles as the launcher.
                if std::fs::write(&dsh_bin, &content).is_ok() {
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

/// The wrapper script a version's `node_modules/.bin/dsh` should hold, or
/// `None` when that version tree does not exist yet. Unix-only by
/// construction: the launcher targets macOS.
#[cfg(unix)]
fn wrapper_script(version_dir: &Path) -> Option<String> {
    let target_bin = version_bin(version_dir);
    if !target_bin.exists() {
        return None;
    }
    Some(format!(
        "#!/bin/sh\nexec node \"{}\" \"$@\"\n",
        target_bin.to_string_lossy()
    ))
}

#[cfg(not(unix))]
fn wrapper_script(_version_dir: &Path) -> Option<String> {
    None
}

/// The PATH directories an instance terminal prepends: the version's `.bin`
/// (via [`ensure_terminal_dsh_wrapper`]), so a bare `dsh` in the terminal is
/// the same CLI the launcher runs. Node itself comes from the user's own
/// toolchain — the launcher no longer ships one.
pub fn terminal_path_dirs(version_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = version_dir.and_then(ensure_terminal_dsh_wrapper) {
        dirs.push(dir);
    }
    dirs
}

/// Where a version's `dsh` command lives: `<版本目录>/node_modules/.bin`.
/// 本机's terminal gets it as a directory the launcher may write a wrapper into
/// ([`ensure_terminal_dsh_wrapper`]); on another machine the same directory is
/// simply put on the terminal's `PATH`, because the launcher writes nothing
/// there and pnpm's own entry needs no flag of ours.
pub fn version_bin_dir(version_dir: &Path) -> PathBuf {
    version_dir.join("node_modules").join(".bin")
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

    fn local_instance() -> crate::config::DshInstance {
        crate::config::DshInstance {
            id: "i1".into(),
            name: "主实例".into(),
            version_id: "v1".into(),
            home_id: "h1".into(),
            host_id: crate::config::LOCAL_HOST_ID.into(),
            env_overrides: [("DSH_TELEMETRY_DISABLED".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        }
    }

    fn remote_instance() -> crate::config::DshInstance {
        crate::config::DshInstance {
            id: "i9".into(),
            name: "服务器实例".into(),
            version_id: "v9".into(),
            home_id: "h9".into(),
            host_id: "host-server".into(),
            env_overrides: [("DSH_TELEMETRY_DISABLED".to_string(), "1".to_string())]
                .into_iter()
                .collect(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        }
    }

    fn remote_host() -> crate::config::Host {
        crate::config::Host {
            id: "host-server".into(),
            display_name: "服务器".into(),
            ssh_target: "zjq_aliyun".into(),
            registry: None,
            extra_env: [(
                "HTTPS_PROXY".to_string(),
                "http://10.0.0.9:3128".to_string(),
            )]
            .into_iter()
            .collect(),
            data_dir: Some(PathBuf::from("/home/demo/.local/share/dsh-launcher")),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    fn config_with_proxy() -> crate::config::Config {
        crate::config::Config {
            homes: vec![
                crate::config::DshHome {
                    id: "h1".into(),
                    name: "本机 HOME".into(),
                    path: PathBuf::from("/Users/me/.dsh"),
                    host_id: crate::config::LOCAL_HOST_ID.into(),
                },
                crate::config::DshHome {
                    id: "h9".into(),
                    name: "服务器 HOME".into(),
                    path: PathBuf::from("/home/demo/.dsh"),
                    host_id: "host-server".into(),
                },
            ],
            versions: Vec::new(),
            instances: vec![local_instance(), remote_instance()],
            hosts: vec![crate::config::Host::local(), remote_host()],
            settings: crate::config::LauncherSettings {
                proxy_enabled: true,
                proxy_apply_dsh: true,
                proxy_url: "http://127.0.0.1".into(),
                proxy_port: 7890,
                ..Default::default()
            },
        }
    }

    fn env_of(env: &[(String, String)], key: &str) -> Option<String> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// 本机's environment is the one the launcher has always built, and the
    /// host-aware form has to leave it exactly as it was — proxy included, and
    /// proxy-over-instance-override included.
    #[test]
    fn the_local_instance_environment_is_what_it_always_was() {
        let cfg = config_with_proxy();
        assert_eq!(
            instance_env(&cfg, "i1").unwrap(),
            instance_env_for(&cfg, "i1", &crate::config::Host::local()).unwrap()
        );

        let env = instance_env(&cfg, "i1").unwrap();
        assert_eq!(env_of(&env, "DSH_HOME").as_deref(), Some("/Users/me/.dsh"));
        assert_eq!(env_of(&env, "DSH_LAUNCHER_INSTANCE").as_deref(), Some("主实例"));
        assert_eq!(env_of(&env, "DSH_TELEMETRY_DISABLED").as_deref(), Some("1"));
        assert_eq!(
            env_of(&env, "HTTPS_PROXY").as_deref(),
            Some("http://127.0.0.1:7890"),
            "本机实例仍带本机代理"
        );
    }

    /// Another machine's instance carries its own machine's facts: that host's
    /// DSH_HOME, the instance's name and own variables, and the host record's
    /// environment variables — and **not** the launcher's proxy, which describes
    /// this Mac's network exit.
    #[test]
    fn a_remote_instance_carries_its_hosts_environment_and_not_the_launchers_proxy() {
        let cfg = config_with_proxy();
        let env = instance_env_for(&cfg, "i9", &remote_host()).unwrap();

        assert_eq!(env_of(&env, "DSH_HOME").as_deref(), Some("/home/demo/.dsh"));
        assert_eq!(env_of(&env, "DSH_LAUNCHER_INSTANCE").as_deref(), Some("服务器实例"));
        assert_eq!(env_of(&env, "DSH_TELEMETRY_DISABLED").as_deref(), Some("1"));
        assert_eq!(
            env_of(&env, "HTTPS_PROXY").as_deref(),
            Some("http://10.0.0.9:3128"),
            "远端的出口由主机记录里的额外环境变量决定"
        );
        assert!(
            !env.iter().any(|(_, v)| v.contains("127.0.0.1:7890")),
            "本机代理一个字节都不该过去：{env:?}"
        );
        assert!(env_of(&env, "NO_PROXY").is_none(), "{env:?}");
    }

    /// DSH_HOME is reserved on both machines: neither an instance override nor a
    /// host's blanket settings may move it away from the record's own value.
    #[test]
    fn dsh_home_is_reserved_on_every_host() {
        let mut cfg = config_with_proxy();
        cfg.instances[1]
            .env_overrides
            .insert("DSH_HOME".into(), "/tmp/hijacked".into());
        cfg.hosts[1]
            .extra_env
            .insert("DSH_HOME".into(), "/tmp/hijacked".into());

        let env = instance_env_for(&cfg, "i9", &remote_host()).unwrap();
        assert_eq!(env_of(&env, "DSH_HOME").as_deref(), Some("/home/demo/.dsh"));
    }

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

        // The interpreter comes from the caller's resolved binding; the test
        // only cares that it is used verbatim as argv[0]'s program.
        let node = Path::new("/opt/test/bin/node");
        let cmd = instance_command(node, &dir, "web", Some(3099)).unwrap();
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program().to_string_lossy(), node.to_string_lossy());
        let argv: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        // Ordering: bin.js first, then the DSH subcommand/flags.
        assert!(argv[0].ends_with("bin.js"));
        assert_eq!(argv[1], "--profile");
        assert_eq!(argv[2], "web");
        assert_eq!(argv[3], "--port");
        assert_eq!(argv[4], "3099");
        // The instance run never passes --host (profile layer owns the bind).
        assert!(!argv.iter().any(|a| a == "--host"));

        // Non-web profile: no --port, no --no-open.
        let plain = instance_command(node, &dir, "bot", None).unwrap();
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

        let cmd = template_boot_command(
            Path::new("/opt/test/bin/node"),
            &dir,
            Path::new("/tmp/home"),
            20001,
        )
        .unwrap();
        let argv: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let host = argv.iter().position(|a| a == "--host").unwrap();
        assert_eq!(argv[host + 1], "127.0.0.1");
        let port = argv.iter().position(|a| a == "--port").unwrap();
        assert_eq!(argv[port + 1], "20001");
        // argv = [bin.js, --profile, web, --host, 127.0.0.1, --port, N].
        assert!(argv[0].ends_with("bin.js"));
        assert_eq!(argv[1], "--profile");
        assert_eq!(argv[2], "web");
        std::fs::remove_dir_all(&dir).ok();
    }
}
