//! Toolchain module — the single owner of "which pnpm, which store, which
//! registry, which network flags".
//!
//! Every pnpm/npm invocation the launcher drives (version installs, source
//! builds, plugin management, version listings) derives its toolchain policy
//! from here, so a policy change (a new pnpm major, different timeouts, a
//! mirror variable) is made once and applies everywhere. Two entry flavors
//! resolve the pnpm: background tasks get task-log notes and a percent nudge
//! ([`ensure_pnpm_for_task`]); interactive paths just app-log
//! ([`ensure_pnpm`]). They share the probe and bootstrap decisions.

use crate::AppState;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

/// Lowest pnpm major the launcher drives installs with.
///
/// This used to be an exact pin on 11, on the theory that "DSH profiles are
/// initialized by pnpm 11" and any other major produces trees the CLI does not
/// expect. That premise did not hold up: DSH's profile init writes only
/// `package.json`, `cordis.patch.yml` and `pnpm-workspace.yaml` and checks no
/// pnpm version anywhere (the `pnpm@11.7.0` in DSH's own repo builds DSH's
/// monorepo, not consumer profiles). Meanwhile every easy install channel —
/// `brew install pnpm`, `npm i -g pnpm`, `curl get.pnpm.io` — now delivers
/// major 12, so an exact pin rejected the toolchain most users already had.
///
/// The floor stays at 11 rather than being dropped: it is the oldest major the
/// launcher's own flag set and store layout are verified against. See
/// ADR-0003.
pub(crate) const MIN_PNPM_MAJOR: u32 = 11;

/// What the user is told to run when no acceptable pnpm is on the machine.
/// The launcher no longer installs one itself: toolchain provisioning is the
/// user's call (nvm/brew/standalone all work), and the setup guide offers this
/// same text as a copyable command. Not pinned to a version — the accept floor
/// is a range, so pinning here would only go stale.
pub(crate) const PNPM_INSTALL_HINT: &str = "npm install -g pnpm";

/// Whether a probe's major version is one the launcher will drive installs
/// with. `None` (unparsable version output) is not acceptable — a shim whose
/// `--version` is unreadable cannot be trusted to run a 600-package tree.
pub(crate) fn accepts_pnpm_major(major: u32) -> bool {
    major >= MIN_PNPM_MAJOR
}

/// Parses the major version out of `pnpm --version` output ("11.17.0\n").
pub(crate) fn pnpm_major(version_output: &str) -> Option<u32> {
    version_output
        .trim()
        .split('.')
        .next()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Optional registry mirror (e.g. npmmirror) via `DSH_NPM_REGISTRY`, trimmed.
/// Shared by every npm/pnpm invocation so listings and installs never
/// disagree about which registry answered.
pub(crate) fn registry_mirror() -> Option<String> {
    std::env::var("DSH_NPM_REGISTRY")
        .ok()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
}

/// The launcher's shared pnpm content store under the data dir, reused by
/// version installs, source builds, and plugin management. Profiles linked
/// against any other store fail with ERR_PNPM_UNEXPECTED_STORE.
pub(crate) fn store_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(".pnpm-store")
}

/// Network robustness settings for pnpm *download* subcommands (`add` /
/// `install`): the defaults (60s timeout, 2 retries) are too tight for large
/// native binaries (e.g. sharp), which abort on flaky connections.
///
/// They travel as `pnpm_config_*` environment variables, NOT as
/// `--config.<name>=<value>` flags. The flag form looks cross-major but is a
/// trap on pnpm 11: pnpm re-parses the collected `--config.` args with an
/// empty type map (`nopt({}, {}, configDotArgs)` in pnpm.mjs), so
/// `--config.fetch-timeout=300000` reaches `AbortSignal.timeout()` as the
/// *string* "300000". Node throws ERR_INVALID_ARG_TYPE, pnpm swallows that
/// rejection, and the install promise never settles: the task sits at its
/// percent ceiling forever having downloaded nothing. That is the
/// stuck-near-100% report -- the progress bar is a heartbeat, not a download.
///
/// Env vars instead go through the config reader, which coerces by declared
/// type (`fetch-timeout` is declared `Number`), so both majors get a number.
/// Verified against pnpm 11.22.0 and 12.4.2.
///
/// `--network-concurrency` has no cross-major spelling at all
/// (`--config.network-concurrency=4` crashes pnpm 11 with "Expected
/// `concurrency` to be a number from 1 and up"), so it is deliberately
/// absent: pnpm's default concurrency is a fine trade for one settings set
/// that works everywhere.
pub(crate) fn pnpm_network_env() -> [(&'static str, &'static str); 3] {
    [
        ("pnpm_config_fetch_timeout", "300000"), // 5 min per request
        ("pnpm_config_fetch_retries", "5"),
        ("pnpm_config_fetch_retry_maxtimeout", "120000"),
    ]
}

/// Applies [`pnpm_network_env`] to a command that downloads through pnpm.
///
/// Env is the only channel that carries these settings intact on pnpm 11 (see
/// [`pnpm_network_env`]), and it survives being inherited: the `dsh plugin`
/// path spawns pnpm itself, so the variables set on the CLI process are what
/// reaches pnpm there.
pub(crate) fn apply_pnpm_network_env(cmd: &mut tokio::process::Command) {
    for (key, value) in pnpm_network_env() {
        cmd.env(key, value);
    }
}

/// The full pnpm argument set for a command that writes to the launcher's
/// shared store: store, log level, optional registry.
///
/// This is the one place those choices are made. Three call sites used to
/// assemble the same four pieces independently (npm version install, source
/// build, plugin management through `dsh plugin`), which is exactly why the
/// pnpm-12 flag migration had to be applied three times by hand. `fetch` is
/// false for subcommands that reject the download flags outright (`pnpm
/// remove` fails with "Unknown options: 'fetch-timeout'" before touching
/// anything).
///
/// The network-robustness settings are NOT here: they must reach pnpm as
/// `pnpm_config_*` env vars (see [`pnpm_network_env`]), which is a property of
/// the command, not of this argument list. Download-capable callers pair this
/// with [`apply_pnpm_network_env`].
pub(crate) fn pnpm_store_flags(store_dir: &Path, loglevel: &str) -> Vec<String> {
    let mut args = vec![
        "--store-dir".to_string(),
        store_dir.to_string_lossy().to_string(),
        format!("--config.loglevel={loglevel}"),
    ];
    if let Some(registry) = registry_mirror() {
        args.push("--registry".to_string());
        args.push(registry);
    }
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
    let mut cmd = tokio::process::Command::new(prog);
    crate::process::hide_console(&mut cmd);
    cmd.current_dir(crate::runtime::PROBE_CWD);
    let out = cmd.arg("--version").output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).to_string();
    let major = pnpm_major(&raw);
    Some((raw, major))
}

/// The pnpm executable the launcher should drive installs with: the first
/// candidate whose major is acceptable, plus its version output. Candidates
/// come from the same scan the setup page reports (see
/// [`crate::runtime::candidate_paths`]), so the two never disagree.
///
/// `note` receives a human-readable reason for each candidate that was
/// skipped, so a user with pnpm 9 learns why it was not used instead of seeing
/// a bare "not found".
pub(crate) async fn resolve_pnpm(
    note: &mut (dyn FnMut(String) + Send),
) -> Option<(PathBuf, String)> {
    for candidate in crate::runtime::candidate_paths("pnpm") {
        if !candidate.is_file() {
            continue;
        }
        let Some((raw, major)) = probe_pnpm_version(&candidate).await else {
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

/// The error every pnpm-dependent flow returns when no acceptable pnpm exists.
/// One shared wording so the version-install task, the source build and plugin
/// management all tell the user the same actionable thing.
pub(crate) fn pnpm_missing_error() -> String {
    format!(
        "未找到可用的 pnpm（需要 {MIN_PNPM_MAJOR} 或更高版本）。\n\
         请在终端执行以下命令后，回到「设置 → 环境」点重新检测：\n\n  {PNPM_INSTALL_HINT}"
    )
}

/// Resolves a pnpm executable for interactive paths (plugin management); notes
/// go to the app log.
pub(crate) async fn ensure_pnpm() -> Result<PathBuf, String> {
    let mut notes = Vec::new();
    match resolve_pnpm(&mut |s| notes.push(s)).await {
        Some((path, version)) => {
            for note in notes {
                crate::log_warn!("{note}");
            }
            crate::log_info!("使用系统 pnpm {}（{}）", version, path.display());
            Ok(path)
        }
        None => {
            for note in notes {
                crate::log_warn!("{note}");
            }
            Err(pnpm_missing_error())
        }
    }
}

/// Resolves a pnpm executable for a background task: the same decision, with
/// the skip reasons also landing in the task log so the user sees why an
/// installed pnpm was not used.
pub(crate) async fn ensure_pnpm_for_task(
    app: &AppHandle,
    state: &State<'_, AppState>,
    task_id: &str,
) -> Result<PathBuf, String> {
    let mut notes = Vec::new();
    match resolve_pnpm(&mut |s| notes.push(s)).await {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // Regression guard for the stuck-install bug: the network settings must
        // NOT be passed as `--config.fetch-*=...`. pnpm 11 re-parses those with
        // an empty type map, so the value arrives as a string and
        // `AbortSignal.timeout()` throws ERR_INVALID_ARG_TYPE. pnpm swallows
        // that rejection, the install promise never settles, and the task hangs
        // at its percent ceiling having downloaded nothing.
        for (key, value) in pnpm_network_env() {
            assert!(
                key.starts_with("pnpm_config_"),
                "network settings must use the pnpm_config_ env prefix: {key}"
            );
            assert!(
                !key.contains('-'),
                "env keys cannot contain dashes: {key}"
            );
            assert!(!value.is_empty(), "missing value for {key}");
        }
        // The three settings the flag set used to carry must all still be here.
        let keys: Vec<&str> = pnpm_network_env().iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"pnpm_config_fetch_timeout"));
        assert!(keys.contains(&"pnpm_config_fetch_retries"));
        assert!(keys.contains(&"pnpm_config_fetch_retry_maxtimeout"));
        // `--network-concurrency` has no cross-major spelling; its absence is
        // deliberate, so guard against it being reintroduced by habit.
        assert!(
            !keys.iter().any(|k| k.contains("concurrency")),
            "network-concurrency cannot be passed to both majors"
        );
    }

    #[test]
    fn accepted_major_is_a_floor_not_an_exact_pin() {
        // DSH itself never pins a pnpm major (its profile init writes only
        // package.json / cordis.patch.yml / pnpm-workspace.yaml), so the
        // launcher accepts anything at or above the verified floor. pnpm 12's
        // tree, lockfile and build-approval flow are compatible; see ADR-0003.
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
        // `pnpm remove` used to reject the fetch flags outright, which is why
        // they are not in this arg list at all any more: env config is not
        // validated per-subcommand. Guard that no `--config.fetch-*` sneaks back
        // in -- that spelling is the stuck-install bug.
        let flags = pnpm_store_flags(Path::new("/s"), "warn");
        assert!(flags.iter().any(|a| a == "--config.loglevel=warn"));
        assert!(!flags.iter().any(|a| a.starts_with("--config.fetch-")));
    }
}
