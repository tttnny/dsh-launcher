use crate::AppState;
use std::path::{Path, PathBuf};
use tauri::State;

pub(crate) const MIN_PNPM_MAJOR: u32 = 11;

pub(crate) const PNPM_INSTALL_HINT: &str = "npm install -g pnpm";

pub(crate) fn accepts_pnpm_major(major: u32) -> bool {
    major >= MIN_PNPM_MAJOR
}

pub(crate) fn pnpm_major(version_output: &str) -> Option<u32> {
    version_output
        .trim()
        .split('.')
        .next()?
        .trim()
        .parse::<u32>()
        .ok()
}

pub(crate) fn registry_mirror() -> Option<String> {
    std::env::var("DSH_NPM_REGISTRY")
        .ok()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
}

pub(crate) fn store_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(".pnpm-store")
}

pub(crate) fn pnpm_network_env() -> [(&'static str, &'static str); 3] {
    [
        ("pnpm_config_fetch_timeout", "300000"),
        ("pnpm_config_fetch_retries", "5"),
        ("pnpm_config_fetch_retry_maxtimeout", "120000"),
    ]
}

pub(crate) fn apply_pnpm_network_env(cmd: &mut tokio::process::Command) {
    for (key, value) in pnpm_network_env() {
        cmd.env(key, value);
    }
}

pub(crate) fn pnpm_store_flags(store_dir: &Path, loglevel: &str) -> Vec<String> {
    pnpm_store_flags_with(store_dir, loglevel, registry_mirror().as_deref())
}

pub(crate) fn pnpm_store_flags_with(
    store_dir: &Path,
    loglevel: &str,
    registry: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--store-dir".to_string(),
        store_dir.to_string_lossy().to_string(),
        format!("--config.loglevel={loglevel}"),
    ];
    if let Some(registry) = registry.map(str::trim).filter(|r| !r.is_empty()) {
        args.push("--registry".to_string());
        args.push(registry.to_string());
    }
    args
}

pub(crate) fn pnpm_install_args(
    dir: &Path,
    store_dir: &Path,
    registry: Option<&str>,
    package: &str,
) -> Vec<String> {
    let mut args = vec![
        "install".to_string(),
        "--prefix".to_string(),
        dir.to_string_lossy().to_string(),
    ];
    args.extend(pnpm_store_flags_with(store_dir, "http", registry));
    args.push(package.to_string());
    args
}

/// Runs `<prog> --version` from a neutral cwd; `Some((raw_output, major))`
/// when the probe succeeded (the major is `None` when the output is not a
/// version).
///
/// The cwd matters: pnpm resolves a `packageManager` field from the working
/// directory and reports *that* version — a corepack shim would even download
/// it — so a probe run from the app's cwd can report a version the binary is
/// not. [`crate::runtime::PROBE_CWD`] makes the answer depend only on the
/// binary.
async fn probe_pnpm_version(prog: &Path) -> Option<(String, Option<u32>)> {
    let raw = crate::runtime::probe_version(prog).await?;
    let major = pnpm_major(&raw);
    Some((raw, major))
}

pub(crate) async fn resolve_pnpm(
    manual: Option<&Path>,
    note: &mut (dyn FnMut(String) + Send),
) -> Option<(PathBuf, String)> {
    let candidates = crate::runtime::tool_candidates("pnpm", manual).await;
    resolve_pnpm_from(candidates, note).await
}

async fn resolve_pnpm_from(
    candidates: Vec<(PathBuf, crate::runtime::ToolSource)>,
    note: &mut (dyn FnMut(String) + Send),
) -> Option<(PathBuf, String)> {
    for (candidate, source) in candidates {
        let is_manual = source == crate::runtime::ToolSource::Manual;
        if !candidate.is_file() {
            if is_manual {
                note(format!(
                    "你指定的 pnpm 不存在：{}，已跳过",
                    candidate.display()
                ));
            }
            continue;
        }
        let Some((raw, major)) = probe_pnpm_version(&candidate).await else {
            if is_manual {
                note(format!(
                    "你指定的 pnpm 无法运行：{}，已跳过",
                    candidate.display()
                ));
            }
            continue;
        };
        match major {
            Some(m) if accepts_pnpm_major(m) => {
                return Some((candidate, raw.trim().to_string()));
            }
            Some(m) => note(format!(
                "{}（pnpm {m}）低于所需的 pnpm {MIN_PNPM_MAJOR}，已跳过",
                candidate.display()
            )),
            None => note(format!(
                "{} 的版本号无法解析（{}），已跳过",
                candidate.display(),
                raw.trim()
            )),
        }
    }
    None
}

pub(crate) fn pnpm_missing_error() -> String {
    format!(
        "未找到可用的 pnpm（需要 {MIN_PNPM_MAJOR} 或更高版本）。\n\
         请在终端执行以下命令后，回到「设置 → 环境」点重新检测：\n\n  {PNPM_INSTALL_HINT}"
    )
}

pub(crate) async fn ensure_pnpm_for_task<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &State<'_, AppState>,
    task_id: &str,
) -> Result<PathBuf, String> {
    let manual = state
        .config
        .lock()
        .unwrap()
        .local_host()
        .and_then(|host| host.toolchain_overrides.pnpm.clone());
    let mut notes = Vec::new();
    match resolve_pnpm(manual.as_deref(), &mut |s| notes.push(s)).await {
        Some((path, version)) => {
            for note in &notes {
                crate::log_warn!("{note}");
                crate::tasks::push_task_log(app, state, task_id, note).await;
            }
            crate::tasks::push_task_log(
                app,
                state,
                task_id,
                &format!("使用 pnpm {version}（{}）", path.display()),
            )
            .await;
            Ok(path)
        }
        None => {
            for note in &notes {
                crate::log_warn!("{note}");
                crate::tasks::push_task_log(app, state, task_id, note).await;
            }
            Err(pnpm_missing_error())
        }
    }
}

/// The pnpm one background task drives on one host, and the directories every
/// command it runs there needs in front of `PATH`.
///
/// The directories are not a convenience: a pnpm that is a standalone binary
/// (pnpm 12 ships as one) runs without node, so the lifecycle scripts it starts
/// inherit whatever `PATH` the ssh session has — and an install that runs
/// `node scripts/postinstall` then fails with exit status 127. The probe
/// resolved node through an interactive login shell, which is not the shell a
/// detached install runs in, so its directory has to travel with the command.
pub(crate) struct PnpmForTask {
    pub path: PathBuf,
    pub path_dirs: Vec<String>,
}

pub(crate) async fn ensure_pnpm_for_task_on<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &State<'_, AppState>,
    task_id: &str,
    host_id: Option<&str>,
) -> Result<PnpmForTask, String> {
    let host = {
        let cfg = state.config.lock().unwrap();
        crate::config::resolve_host(&cfg, host_id)?.clone()
    };
    if host.builtin {
        let path = ensure_pnpm_for_task(app, state, task_id).await?;
        return Ok(PnpmForTask {
            path,
            path_dirs: Vec::new(),
        });
    }

    let tools = crate::runtime::remote_host_tools(state, &host).await?;
    for skipped in &tools.env.pnpm.skipped {
        crate::tasks::push_task_log(
            app,
            state,
            task_id,
            &crate::runtime::skipped_candidate_note("pnpm", skipped),
        )
        .await;
    }
    let path_dirs = crate::runtime::tool_path_dirs(
        tools.env.node.path.as_deref().map(Path::new),
        tools.pnpm.as_ref().map(|bound| bound.path.as_path()),
    );
    match tools.pnpm {
        Some(bound) => {
            if let Some(node) = tools.env.node.path.as_deref() {
                crate::tasks::push_task_log(
                    app,
                    state,
                    task_id,
                    &format!("依赖构建脚本使用那台机器上的 node（{node}）"),
                )
                .await;
            }
            crate::tasks::push_task_log(
                app,
                state,
                task_id,
                &format!(
                    "使用那台机器上的 pnpm {}（{}）",
                    bound.version,
                    bound.path.display()
                ),
            )
            .await;
            Ok(PnpmForTask {
                path: bound.path,
                path_dirs,
            })
        }
        None => Err(crate::runtime::remote_pnpm_missing_error(
            &host,
            &tools.target,
            &tools.env.managers,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ToolSource;

    fn fake_pnpm(tag: &str, version: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("pnpm");
        std::fs::write(&exe, format!("#!/bin/sh\necho {version}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (dir, exe)
    }

    #[tokio::test]
    async fn the_hand_picked_pnpm_is_the_one_install_flows_drive() {
        let (dir, exe) = fake_pnpm("pnpm-manual", "12.4.2");
        let mut notes = Vec::new();
        let picked = resolve_pnpm_from(
            vec![
                (exe.clone(), ToolSource::Manual),
                (dir.join("missing"), ToolSource::System),
            ],
            &mut |s| notes.push(s),
        )
        .await;
        assert_eq!(picked, Some((exe, "12.4.2".to_string())));
        assert!(notes.is_empty(), "nothing was skipped: {notes:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_unusable_hand_picked_pnpm_falls_back_and_says_so() {
        let (old_dir, old) = fake_pnpm("pnpm-old", "9.15.0");
        let (good_dir, good) = fake_pnpm("pnpm-good", "12.4.2");
        let mut notes = Vec::new();
        let picked = resolve_pnpm_from(
            vec![
                (old.clone(), ToolSource::Manual),
                (good.clone(), ToolSource::System),
            ],
            &mut |s| notes.push(s),
        )
        .await;
        assert_eq!(picked, Some((good, "12.4.2".to_string())));
        assert!(
            notes
                .iter()
                .any(|n| n.contains("pnpm 9") && n.contains(&old.display().to_string())),
            "the user has to be told why their pnpm was passed over: {notes:?}"
        );

        let mut notes = Vec::new();
        let gone = old_dir.join("gone");
        assert!(
            resolve_pnpm_from(vec![(gone.clone(), ToolSource::Manual)], &mut |s| notes
                .push(s))
            .await
            .is_none()
        );
        assert!(
            notes
                .iter()
                .any(|n| n.contains(&gone.display().to_string())),
            "a stale pick must be named: {notes:?}"
        );

        std::fs::remove_dir_all(&old_dir).ok();
        std::fs::remove_dir_all(&good_dir).ok();
    }

    #[test]
    fn pnpm_major_parses_version_output() {
        assert_eq!(pnpm_major("11.17.0\n"), Some(11));
        assert_eq!(pnpm_major("  10.4.1  "), Some(10));
        assert_eq!(pnpm_major("12.0.0-beta.1"), Some(12));
        assert_eq!(pnpm_major(""), None);
        assert_eq!(pnpm_major("not-a-version"), None);
    }

    #[test]
    fn network_settings_ride_as_env_not_config_flags() {
        for (key, value) in pnpm_network_env() {
            assert!(
                key.starts_with("pnpm_config_"),
                "network settings must use the pnpm_config_ env prefix: {key}"
            );
            assert!(!key.contains('-'), "env keys cannot contain dashes: {key}");
            assert!(!value.is_empty(), "missing value for {key}");
        }
        let keys: Vec<&str> = pnpm_network_env().iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"pnpm_config_fetch_timeout"));
        assert!(keys.contains(&"pnpm_config_fetch_retries"));
        assert!(keys.contains(&"pnpm_config_fetch_retry_maxtimeout"));
        assert!(
            !keys.iter().any(|k| k.contains("concurrency")),
            "network-concurrency cannot be passed to both majors"
        );
    }

    #[test]
    fn accepted_major_is_a_floor_not_an_exact_pin() {
        assert!(accepts_pnpm_major(11));
        assert!(accepts_pnpm_major(12));
        assert!(accepts_pnpm_major(13));
        assert!(!accepts_pnpm_major(10));
        assert!(!accepts_pnpm_major(9));
    }

    #[test]
    fn store_flags_carry_store_and_loglevel_always() {
        let store = Path::new("/data/.pnpm-store");
        let flags = pnpm_store_flags(store, "http");
        assert!(flags.iter().any(|a| a == "--store-dir"));
        assert!(flags.iter().any(|a| a == "/data/.pnpm-store"));
        assert!(flags.iter().any(|a| a == "--config.loglevel=http"));
    }

    #[test]
    fn store_flags_never_carry_network_flags() {
        let flags = pnpm_store_flags(Path::new("/s"), "warn");
        assert!(flags.iter().any(|a| a == "--config.loglevel=warn"));
        assert!(!flags.iter().any(|a| a.starts_with("--config.fetch-")));
    }

    #[test]
    fn the_install_arguments_are_the_same_whichever_machine_runs_them() {
        let args = pnpm_install_args(
            Path::new("/versions/1.2.3"),
            Path::new("/store"),
            Some("https://mirror.example/npm"),
            "@deepseek-ai/dsh@1.2.3",
        );
        assert_eq!(
            args,
            vec![
                "install",
                "--prefix",
                "/versions/1.2.3",
                "--store-dir",
                "/store",
                "--config.loglevel=http",
                "--registry",
                "https://mirror.example/npm",
                "@deepseek-ai/dsh@1.2.3",
            ]
        );

        assert_eq!(
            pnpm_install_args(
                Path::new("/versions/1.2.3"),
                Path::new("/store"),
                None,
                "@deepseek-ai/dsh@1.2.3"
            ),
            vec![
                "install",
                "--prefix",
                "/versions/1.2.3",
                "--store-dir",
                "/store",
                "--config.loglevel=http",
                "@deepseek-ai/dsh@1.2.3",
            ]
        );

        assert_eq!(
            pnpm_store_flags_with(Path::new("/store"), "http", Some("  ")),
            pnpm_store_flags_with(Path::new("/store"), "http", None)
        );
    }
}
