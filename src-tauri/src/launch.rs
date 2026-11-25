use crate::config::Config;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub fn is_repo_checkout(version_dir: &Path) -> bool {
    version_dir
        .join("apps")
        .join("cli")
        .join("package.json")
        .exists()
}

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

pub fn web_app_supports_no_open(version_dir: &Path) -> bool {
    match std::fs::read_to_string(web_app_startup_script(version_dir)) {
        Ok(raw) => supports_no_open(&raw),
        Err(_) => false,
    }
}

pub fn web_app_startup_script(version_dir: &Path) -> PathBuf {
    if is_repo_checkout(version_dir) {
        version_dir
            .join("packages")
            .join("bundle")
            .join("web-app")
            .join("lib")
            .join("startup.js")
    } else {
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

pub fn supports_no_open(startup_script: &str) -> bool {
    startup_script.contains("--no-open") || startup_script.contains("no-open")
}

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

pub fn instance_env(cfg: &Config, instance_id: &str) -> Result<Vec<(String, String)>, String> {
    instance_env_for(cfg, instance_id, &crate::config::Host::local())
}

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
            continue;
        }
        env.push((k.clone(), v.clone()));
    }
    if host.builtin {
        if cfg.settings.proxy_enabled && cfg.settings.proxy_apply_dsh {
            crate::proxy::override_env(&mut env, &cfg.settings);
        }
    } else {
        for (k, v) in &host.extra_env {
            if k == "DSH_HOME" {
                continue;
            }
            env.push((k.clone(), v.clone()));
        }
    }
    Ok(env)
}

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

pub fn template_boot_command(
    node: &Path,
    version_dir: &Path,
    home_path: &Path,
    port: u16,
) -> Result<Command, String> {
    let mut cmd = dsh_base(node, version_dir)?;
    cmd.args(template_boot_flags(port))
        .env("DSH_HOME", home_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    Ok(cmd)
}

pub fn template_boot_argv(node: &Path, version_dir: &Path, port: u16) -> Vec<String> {
    let mut argv = vec![
        node.to_string_lossy().to_string(),
        version_bin(version_dir).to_string_lossy().to_string(),
    ];
    argv.extend(template_boot_flags(port));
    argv
}

fn template_boot_flags(port: u16) -> Vec<String> {
    vec![
        "--profile".to_string(),
        "web".to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
    ]
}

pub fn ensure_terminal_dsh_wrapper(version_dir: &Path) -> Option<PathBuf> {
    let bin_dir = version_bin_dir(version_dir);
    let dsh_bin = bin_dir.join("dsh");

    let desired = wrapper_script(version_dir);
    if let Some(content) = desired {
        let existing = std::fs::read_to_string(&dsh_bin).ok();
        let wrapper_is_ours = existing
            .as_deref()
            .is_some_and(|raw| raw.starts_with("#!/bin/sh\nexec node "));
        if (!dsh_bin.exists() || wrapper_is_ours) && existing.as_deref() != Some(content.as_str()) {
            let _ = std::fs::create_dir_all(&bin_dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
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

pub fn terminal_path_dirs(version_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = version_dir.and_then(ensure_terminal_dsh_wrapper) {
        dirs.push(dir);
    }
    dirs
}

pub fn version_bin_dir(version_dir: &Path) -> PathBuf {
    version_dir.join("node_modules").join(".bin")
}

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
        (
            "DSH_LAUNCHER_INSTANCE".to_string(),
            instance_name.to_string(),
        ),
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

    #[test]
    fn the_local_instance_environment_is_what_it_always_was() {
        let cfg = config_with_proxy();
        assert_eq!(
            instance_env(&cfg, "i1").unwrap(),
            instance_env_for(&cfg, "i1", &crate::config::Host::local()).unwrap()
        );

        let env = instance_env(&cfg, "i1").unwrap();
        assert_eq!(env_of(&env, "DSH_HOME").as_deref(), Some("/Users/me/.dsh"));
        assert_eq!(
            env_of(&env, "DSH_LAUNCHER_INSTANCE").as_deref(),
            Some("主实例")
        );
        assert_eq!(env_of(&env, "DSH_TELEMETRY_DISABLED").as_deref(), Some("1"));
        assert_eq!(
            env_of(&env, "HTTPS_PROXY").as_deref(),
            Some("http://127.0.0.1:7890"),
            "本机实例仍带本机代理"
        );
    }

    #[test]
    fn a_remote_instance_carries_its_hosts_environment_and_not_the_launchers_proxy() {
        let cfg = config_with_proxy();
        let env = instance_env_for(&cfg, "i9", &remote_host()).unwrap();

        assert_eq!(env_of(&env, "DSH_HOME").as_deref(), Some("/home/demo/.dsh"));
        assert_eq!(
            env_of(&env, "DSH_LAUNCHER_INSTANCE").as_deref(),
            Some("服务器实例")
        );
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
        assert!(!web_app_supports_no_open(&dir));
        std::fs::create_dir_all(&startup_dir).unwrap();
        std::fs::write(
            startup_dir.join("startup.js"),
            "const p = new Command().option('--host <host>').option('--port <port>')",
        )
        .unwrap();
        assert!(!web_app_supports_no_open(&dir));
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
        assert!(version_bin(&dir).ends_with(
            std::path::Path::new("node_modules")
                .join("@deepseek-ai")
                .join("dsh")
                .join("lib")
                .join("bin.js")
        ));
        assert!(!version_bin_ready(&dir));
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
        let dir = std::env::temp_dir().join(format!("dsh-launch-test-{}", uuid::Uuid::new_v4()));
        let bin = dir
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("bin.js"), "// bin").unwrap();

        let node = Path::new("/opt/test/bin/node");
        let cmd = instance_command(node, &dir, "web", Some(3099)).unwrap();
        let std_cmd = cmd.as_std();
        assert_eq!(
            std_cmd.get_program().to_string_lossy(),
            node.to_string_lossy()
        );
        let argv: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(argv[0].ends_with("bin.js"));
        assert_eq!(argv[1], "--profile");
        assert_eq!(argv[2], "web");
        assert_eq!(argv[3], "--port");
        assert_eq!(argv[4], "3099");
        assert!(!argv.iter().any(|a| a == "--host"));

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
        assert!(argv[0].ends_with("bin.js"));
        assert_eq!(argv[1], "--profile");
        assert_eq!(argv[2], "web");
        std::fs::remove_dir_all(&dir).ok();
    }
}
