use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::State;

use crate::AppState;

/// Where a resolved executable came from: the user pointed at it, the user's
/// login shell has it on PATH, or the launcher found it in a known install
/// directory on its own. The setup page reports this so "环境已就绪" can say
/// which of the three it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolSource {
    Manual,
    System,
    Fallback,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    /// Which candidate the executable at `path` came from; `None` when the tool
    /// was not found at all.
    pub source: Option<ToolSource>,
    /// The path the user picked by hand, if any. Reported even when it can no
    /// longer be run, so the page can say that the choice has stopped working
    /// instead of silently showing a different tool.
    pub manual_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeStatus {
    /// Which host this answer is about — `host-local` for 本机. A page that
    /// switched hosts can tell an answer that arrived late from the one it is
    /// waiting for.
    pub host_id: String,
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
    /// The shell the system environment was read from (e.g. `/bin/zsh`, or the
    /// remote machine's `bash`), or `None` when that read failed — in which case
    /// the system environment falls back to the PATH the app itself inherited.
    pub system_env_shell: Option<String>,
    /// Candidates the probe passed over, with the reason. A remote host passes
    /// over a Node below the recommended major (and any pnpm below its floor)
    /// and reports it; the local host passes nothing over (ADR-0005 追加 2).
    pub skipped_candidates: Vec<SkippedCandidate>,
}

/// One candidate the launcher's probe passed over on its way to a conclusion.
#[derive(Clone, Debug, Serialize)]
pub struct SkippedCandidate {
    /// `node` | `pnpm`.
    pub tool: String,
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// A code, not a sentence: `below-minimum` (under the floor the launcher
    /// drives), `unparsable` (a version it cannot read) or `unusable` (a path
    /// the user typed that is not there or would not run). The page says it in
    /// the reader's language.
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub major: Option<u64>,
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

    /// The same numbers and the same manager preference, as commands the user
    /// runs **on this Mac** to install the tool on `host`. One command each,
    /// prefixed with `ssh <host>`, in the shape that installs into the same
    /// directories the probe searches (ADR-0005 追加 4).
    pub(crate) fn for_remote(host: &crate::ssh::RemoteHost, managers: &[String]) -> Self {
        Self {
            min_pnpm_major: crate::toolchain::MIN_PNPM_MAJOR,
            recommended_node_major: RECOMMENDED_NODE_MAJOR,
            pnpm_install_command: crate::remote::ssh_command(
                &host.target,
                crate::remote::pnpm_install_recipe(),
            ),
            node_install_command: crate::remote::ssh_command(
                &host.target,
                crate::remote::node_install_recipe(managers),
            ),
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

/// Every executable of this name the launcher may run, in resolution order, each
/// tagged with where it came from: what the user picked by hand first, then the
/// directories of the system environment (the user's login shell PATH), then the
/// known tool directories. A path that appears twice keeps only its first —
/// earlier entries are the more authoritative source.
///
/// Shared by the UI probe and by [`crate::toolchain`]'s resolver, so "what the
/// setup page reports" and "what the launcher actually executes" can never
/// disagree.
pub(crate) fn candidates(
    program: &str,
    manual: Option<&Path>,
    system_dirs: &[PathBuf],
) -> Vec<(PathBuf, ToolSource)> {
    candidates_with(program, manual, system_dirs, &tool_fallback_bins())
}

/// [`candidates`] with the known tool directories injected, so a test can drive
/// the resolution without touching the machine's own node.
pub(crate) fn candidates_with(
    program: &str,
    manual: Option<&Path>,
    system_dirs: &[PathBuf],
    fallback_dirs: &[PathBuf],
) -> Vec<(PathBuf, ToolSource)> {
    let mut out: Vec<(PathBuf, ToolSource)> = Vec::new();
    if let Some(manual) = manual {
        out.push((manual.to_path_buf(), ToolSource::Manual));
    }
    out.extend(
        system_dirs
            .iter()
            .map(|dir| (exe_in(dir, program), ToolSource::System)),
    );

    for dir in fallback_dirs {
        let exe = exe_in(dir, program);
        // A system directory may be a per-shell temporary: fnm hands every shell
        // its own `fnm_multishells/<pid>_<stamp>/bin`, holding hard links to the
        // version it selected, so a binding made there dies with that shell while
        // the same binary also sits in fnm's durable directory. When a system
        // entry and a known directory hold the *same file*, the durable spelling
        // is the one that gets bound; the source stays `System`, because the
        // user's terminal does reach this very file.
        let same = out
            .iter()
            .position(|(seen, source)| *source == ToolSource::System && same_file(seen, &exe));
        match same {
            Some(index) => out[index].0 = exe,
            None => out.push((exe, ToolSource::Fallback)),
        }
    }

    // The same path can be reached twice (a directory named twice on the PATH,
    // or the system environment naming one of the known directories). The first
    // entry keeps its source.
    let mut seen: Vec<PathBuf> = Vec::new();
    out.retain(|(exe, _)| {
        let duplicate = seen.iter().any(|s| crate::config::paths_equal(s, exe));
        seen.push(exe.clone());
        !duplicate
    });
    out
}

fn exe_in(dir: &Path, program: &str) -> PathBuf {
    // An empty PATH entry means the current directory.
    if dir.as_os_str().is_empty() {
        PathBuf::from(program)
    } else {
        dir.join(program)
    }
}

/// [`candidates`] with the system environment resolved for this call. Async
/// because the system directories are read from the user's login shell (unless
/// that read is still cached).
pub(crate) async fn tool_candidates(
    program: &str,
    manual: Option<&Path>,
) -> Vec<(PathBuf, ToolSource)> {
    let env = system_env(false).await;
    candidates(program, manual, &env.dirs)
}

/// Whether two paths name the same file on disk rather than the same string.
/// On Unix that is device and inode, so hard links count (fnm's per-shell
/// directories are hard links into the real installation) while an ordinary copy
/// does not.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

/// Which source produced the executable at `path`, decided by the candidate
/// list itself: the first entry that is that same file wins, so the answer can
/// never contradict what the resolver actually walked. A bound path that today's
/// list spells differently — the same tool reached through another directory, or
/// the per-shell temporary an earlier read named — still matches by file.
pub(crate) fn source_of(
    program: &str,
    path: &Path,
    manual: Option<&Path>,
    system_dirs: &[PathBuf],
) -> ToolSource {
    let list = candidates(program, manual, system_dirs);
    if let Some((_, source)) = list
        .iter()
        .find(|(exe, _)| crate::config::paths_equal(exe, path))
    {
        return *source;
    }
    if let Some((_, source)) = list.iter().find(|(exe, _)| same_file(exe, path)) {
        return *source;
    }
    ToolSource::Fallback
}

/// Runs `<exe> --version` from a neutral cwd and returns its trimmed stdout, or
/// `None` when it cannot be run or exits non-zero.
///
/// The cwd matters: pnpm resolves a `packageManager` field from the working
/// directory and reports *that* version — a corepack shim would even download
/// it — so [`PROBE_CWD`] is what makes the reported version depend only on the
/// binary.
pub(crate) async fn probe_version(exe: &Path) -> Option<String> {
    let mut cmd = tokio::process::Command::new(exe);
    crate::process::hide_console(&mut cmd);
    cmd.current_dir(PROBE_CWD);
    cmd.stdin(std::process::Stdio::null());
    let out = cmd.arg("--version").output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The first runnable executable of this name, in [`candidates`] order.
async fn probe_tool(program: &str, manual: Option<&Path>) -> ToolStatus {
    // GUI-launched apps inherit a minimal PATH with no shell rc, so a plain
    // Command::new(program) misses fnm/nvm/Homebrew/user-installed tools.
    // The candidate list walks the user's shell PATH first and then the known
    // tool directories (fnm/nvm layouts, PNPM_HOME, Homebrew, …).
    let manual_path = manual.map(|p| p.to_string_lossy().into_owned());
    for (exe, source) in tool_candidates(program, manual).await {
        if !exe.is_file() {
            continue;
        }
        if let Some(version) = probe_version(&exe).await {
            return ToolStatus {
                installed: true,
                version: Some(version),
                path: Some(exe.to_string_lossy().into_owned()),
                source: Some(source),
                manual_path,
            };
        }
    }
    ToolStatus {
        installed: false,
        version: None,
        path: None,
        source: None,
        manual_path,
    }
}

/// How long a shell read is reused. The setup page asks for a status every time
/// it is opened or focused, and each read costs a full interactive shell
/// startup, so the answer is cached briefly instead of spawning one per call.
const SYSTEM_ENV_TTL: Duration = Duration::from_secs(30);

/// A shell that hangs — a `.zshrc` waiting on input, a slow plugin manager —
/// must not hang the launcher with it.
const SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

const SHELL_PATH_MARK: &str = "<<<DSH-PATH>>>";
const SHELL_PATH_END: &str = "<<<DSH-END>>>";

/// Prints PATH between two markers. `.zshrc` files routinely print something of
/// their own (plugin manager banners, version notices), so the whole stdout is
/// not usable as the PATH — only what sits between the markers is.
const SHELL_PATH_SCRIPT: &str = "printf '<<<DSH-PATH>>>%s<<<DSH-END>>>' \"$PATH\"";

/// The system environment as the launcher sees it: the directories a name
/// resolves in, and the shell they were read from (`None` when the read failed
/// and `dirs` are the PATH this app inherited instead).
#[derive(Clone, Debug)]
struct SystemEnv {
    dirs: Vec<PathBuf>,
    shell: Option<String>,
    at: Instant,
}

/// The system environment (PATH) **per host**, keyed by host id: this Mac's comes
/// from the user's own login shell, a remote host's from the probe it runs over
/// ssh. One cache per host is what keeps two machines' conclusions from answering
/// for each other — a single slot would report whichever host was asked last
/// (ADR-0005 追加). Only 本机 has an entry here: a remote host's PATH comes back
/// inside that host's probe answer and is cached with it.
fn system_env_cache() -> &'static Mutex<HashMap<String, SystemEnv>> {
    static CACHE: OnceLock<Mutex<HashMap<String, SystemEnv>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The whole environment answer per remote host, keyed by
/// [`crate::ssh::RemoteHost::id`]. The probe is one ssh call, but the shell on
/// the other side starts an interactive login shell before it says anything, so
/// the answer is reused for the same short window the local PATH read is.
fn remote_env_cache() -> &'static Mutex<HashMap<String, (crate::remote::RemoteEnvironment, Instant)>>
{
    static CACHE: OnceLock<Mutex<HashMap<String, (crate::remote::RemoteEnvironment, Instant)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The PATH this process inherited at launch, captured before
/// [`ensure_macos_paths`] appends the known tool directories to the process
/// PATH. Afterwards `std::env::var("PATH")` is no longer the inherited value,
/// so this is the only honest record of it.
static INHERITED_PATH: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Which host an environment answer is about, and how to reach it. Everything
/// that probes or resolves is stated per host, so no two machines can share a
/// conclusion (ADR-0005 追加).
pub(crate) enum EnvHost {
    /// This Mac: read directly, dial nothing.
    Local,
    /// Another machine: everything goes through the transport.
    Remote(crate::ssh::RemoteHost),
}

/// The host a runtime question is about — the one the caller named, or 本机 when
/// it named none, which is what every caller written before hosts existed meant.
pub(crate) fn env_host(
    state: &AppState,
    host_id: Option<&str>,
) -> Result<(crate::config::Host, EnvHost), String> {
    let host = {
        let cfg = state.config.lock().unwrap();
        crate::config::resolve_host(&cfg, host_id)?.clone()
    };
    if host.builtin {
        return Ok((host, EnvHost::Local));
    }
    let target = crate::ssh::RemoteHost::of(&host, &state.data_dir).map_err(|e| e.message)?;
    Ok((host, EnvHost::Remote(target)))
}

/// Remembers the inherited PATH. Called before the process PATH is first
/// mutated; a later caller gets whatever was stored then.
fn capture_inherited_path() {
    INHERITED_PATH.get_or_init(|| {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect()
    });
}

fn inherited_path_dirs() -> Vec<PathBuf> {
    capture_inherited_path();
    INHERITED_PATH.get().cloned().unwrap_or_default()
}

fn cached_system_env() -> Option<SystemEnv> {
    system_env_cache()
        .lock()
        .unwrap()
        .get(crate::config::LOCAL_HOST_ID)
        .filter(|cached| cached.at.elapsed() < SYSTEM_ENV_TTL)
        .cloned()
}

/// Reads `PATH` from the *local* machine's login shell, cached for
/// [`SYSTEM_ENV_TTL`]. `force` skips the cache (the setup page's re-check: the
/// user just edited their shell config and wants that honoured now).
///
/// A failed read is not an error the user has to act on: the launcher keeps
/// working with the PATH it inherited, and the failure is logged instead.
async fn system_env(force: bool) -> SystemEnv {
    if !force {
        if let Some(cached) = cached_system_env() {
            return cached;
        }
    }
    let env = match read_shell_path().await {
        Ok((shell, path)) => SystemEnv {
            dirs: std::env::split_paths(&path).collect(),
            shell: Some(shell),
            at: Instant::now(),
        },
        Err(reason) => {
            crate::log_warn!("读取系统环境变量失败（{reason}），改用应用继承的 PATH");
            SystemEnv {
                dirs: inherited_path_dirs(),
                shell: None,
                at: Instant::now(),
            }
        }
    };
    remember_system_env(env.dirs.clone(), env.shell.clone());
    env
}

/// Records the local host's system environment, as [`read_shell_path`] read it.
fn remember_system_env(dirs: Vec<PathBuf>, shell: Option<String>) {
    system_env_cache().lock().unwrap().insert(
        crate::config::LOCAL_HOST_ID.to_string(),
        SystemEnv {
            dirs,
            shell,
            at: Instant::now(),
        },
    );
}

/// The whole environment answer of one remote host, probed at most once per
/// [`SYSTEM_ENV_TTL`]. `force` bypasses the cache — 「重新检测」 means the user
/// changed something on that machine and wants that honoured now.
async fn remote_env(
    state: &AppState,
    host: &crate::ssh::RemoteHost,
    overrides: &crate::config::ToolchainOverrides,
    force: bool,
) -> Result<crate::remote::RemoteEnvironment, String> {
    if !force {
        let cached = remote_env_cache()
            .lock()
            .unwrap()
            .get(&host.id)
            .filter(|(_, at)| at.elapsed() < SYSTEM_ENV_TTL)
            .map(|(env, _)| env.clone());
        if let Some(env) = cached {
            return Ok(env);
        }
    }
    let env = crate::remote::environment(
        &*state.host_transport,
        host,
        overrides
            .node
            .as_deref()
            .map(|p| p.to_string_lossy())
            .as_deref(),
        overrides
            .pnpm
            .as_deref()
            .map(|p| p.to_string_lossy())
            .as_deref(),
    )
    .await
    .map_err(|e| e.message)?;
    remote_env_cache()
        .lock()
        .unwrap()
        .insert(host.id.clone(), (env.clone(), Instant::now()));
    Ok(env)
}

/// Drops one host's cached answers. Every write that changes what the probe
/// would find — a new hand-picked path, a cleared one — has to drop them, or the
/// page would keep showing the conclusion of the previous choice.
pub(crate) fn forget_host_env(host_id: &str) {
    system_env_cache().lock().unwrap().remove(host_id);
    remote_env_cache().lock().unwrap().remove(host_id);
}

/// The shell whose environment counts as "the system's": the user's login shell
/// if it is known, otherwise the platform default.
fn login_shell() -> String {
    match std::env::var("SHELL") {
        Ok(shell) if !shell.trim().is_empty() => shell.trim().to_string(),
        _ => {
            if cfg!(target_os = "macos") {
                "/bin/zsh".to_string()
            } else {
                "/bin/bash".to_string()
            }
        }
    }
}

/// Reads PATH out of the user's login shell: `Ok((shell, path))`, or a reason
/// why the read failed.
///
/// A GUI app inherits neither the shell's PATH nor its rc files, so without
/// this a user who exports PATH in `.zshrc` — nvm, fnm, a custom Node — is
/// invisible to the launcher even though the same command works in their
/// terminal.
async fn read_shell_path() -> Result<(String, String), String> {
    let shell = login_shell();
    let mut cmd = tokio::process::Command::new(&shell);
    crate::process::hide_console(&mut cmd);
    cmd.arg("-ilc").arg(SHELL_PATH_SCRIPT);
    cmd.stdin(std::process::Stdio::null());
    // The timeout below drops the future; killing on drop is what keeps a hung
    // shell from being left behind as an orphan process.
    cmd.kill_on_drop(true);

    let output = match tokio::time::timeout(SHELL_PROBE_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(format!("无法启动 {shell}：{e}")),
        Err(_) => {
            return Err(format!(
                "{shell} 没有在 {} 秒内返回",
                SHELL_PROBE_TIMEOUT.as_secs()
            ))
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    match parse_shell_path(&stdout) {
        Some(path) => Ok((shell, path)),
        None => Err(format!("{shell} 的输出里没有 PATH 标记")),
    }
}

/// Pulls PATH out of a shell's stdout: everything between
/// [`SHELL_PATH_MARK`] and [`SHELL_PATH_END`], regardless of whatever else the
/// shell printed. `None` when either marker is missing or the value is empty.
fn parse_shell_path(stdout: &str) -> Option<String> {
    let after_mark = &stdout[stdout.find(SHELL_PATH_MARK)? + SHELL_PATH_MARK.len()..];
    let path = after_mark[..after_mark.find(SHELL_PATH_END)?].trim();
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

/// Probes node and pnpm and records the result in the **local host's** own
/// binding, so every later spawn can use an absolute path instead of
/// re-resolving a name against whatever PATH the app inherited. Returns the
/// binding that was written.
///
/// Freshness is checked per tool: a binding whose path still exists on disk is
/// kept, so a startup probe does not re-run `--version` for a toolchain that
/// has not moved. A binding whose path vanished (an nvm version the user
/// switched away from, a removed brew formula) is re-probed and updated — that
/// is the whole point of persisting a *resolution* rather than a preference.
/// A tool the user picked by hand is only fresh while the binding is exactly
/// that file, so the choice is applied on the next refresh.
///
/// `force` probes even a binding that is still fresh: it is the setup page's
/// re-check, and the user asking for it means the environment may have moved
/// under us (a path exported in `.zshrc` outranks the directory a still-valid
/// binding was found in).
pub async fn refresh_toolchain_binding(
    state: &State<'_, AppState>,
    force: bool,
) -> crate::config::ToolchainBinding {
    refresh_local_binding(state, force).await
}

/// [`refresh_toolchain_binding`] for callers holding `&AppState`, which is every
/// caller inside this module and every test.
pub(crate) async fn refresh_local_binding(
    state: &AppState,
    force: bool,
) -> crate::config::ToolchainBinding {
    let (current, overrides) = local_toolchain(state);
    // Both freshness checks run before either field is moved out.
    let (node_fresh, pnpm_fresh) = (
        !force && current.node_is_fresh(overrides.node.as_deref()),
        !force && current.pnpm_is_fresh(overrides.pnpm.as_deref()),
    );

    // node: keep the recorded path when it still exists.
    let node = if node_fresh {
        current.node
    } else {
        bound_from(probe_tool("node", overrides.node.as_deref()).await)
    };
    // pnpm: reuse the resolver, so the bound binary is exactly the one the
    // install flows will drive (and its version already meets the floor).
    let pnpm = if pnpm_fresh {
        current.pnpm
    } else {
        let mut notes = Vec::new();
        let resolved =
            crate::toolchain::resolve_pnpm(overrides.pnpm.as_deref(), &mut |s| notes.push(s)).await;
        match resolved {
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
    write_host_binding(state, crate::config::LOCAL_HOST_ID, &binding);
    binding
}

/// The local host's recorded binding and the user's choices for it, so every
/// caller reads the same pair.
fn local_toolchain(
    state: &AppState,
) -> (
    crate::config::ToolchainBinding,
    crate::config::ToolchainOverrides,
) {
    let cfg = state.config.lock().unwrap();
    match cfg.local_host() {
        Some(host) => (host.toolchain.clone(), host.toolchain_overrides.clone()),
        None => Default::default(),
    }
}

/// Probes one remote host and records the conclusion in that host's own
/// binding. The one place a remote conclusion is written, so the page and a
/// spawn on that machine read the same resolution — and the same candidates
/// were walked to get it.
async fn remote_probe(
    state: &AppState,
    host: &crate::config::Host,
    target: &crate::ssh::RemoteHost,
    force: bool,
) -> Result<crate::remote::RemoteEnvironment, String> {
    let env = remote_env(state, target, &host.toolchain_overrides, force).await?;
    write_host_binding(state, &host.id, &binding_of_remote(&env));
    Ok(env)
}

/// What a remote probe's answer becomes for the launcher's own use: the path to
/// execute and the version it reported, or `None` when that machine has no
/// usable tool.
fn binding_of_remote(env: &crate::remote::RemoteEnvironment) -> crate::config::ToolchainBinding {
    let bound = |tool: &crate::remote::RemoteToolStatus| match (
        tool.path.as_deref(),
        tool.version.as_deref(),
    ) {
        (Some(path), Some(version)) => Some(crate::config::BoundTool {
            path: PathBuf::from(path),
            version: version.to_string(),
        }),
        _ => None,
    };
    crate::config::ToolchainBinding {
        node: bound(&env.node),
        pnpm: bound(&env.pnpm),
    }
}

/// One remote host's toolchain as a task should drive it: the machine, that
/// machine's probe answer, and the tools the launcher resolved there.
///
/// A task resolves through the same walk the environment page shows, so the
/// binary it runs is the one the page reported (ADR-0005 追加 1).
pub(crate) struct HostTools {
    pub target: crate::ssh::RemoteHost,
    pub env: crate::remote::RemoteEnvironment,
    pub pnpm: Option<crate::config::BoundTool>,
}

/// Resolves one **remote** host's tools on that machine.
pub(crate) async fn remote_host_tools(
    state: &AppState,
    host: &crate::config::Host,
) -> Result<HostTools, String> {
    let target = crate::ssh::RemoteHost::of(host, &state.data_dir).map_err(|e| e.message)?;
    let env = remote_probe(state, host, &target, false).await?;
    let binding = binding_of_remote(&env);
    Ok(HostTools {
        target,
        env,
        pnpm: binding.pnpm,
    })
}

/// Why a version cannot be installed on one remote host: that machine has no
/// pnpm the launcher can drive. The fix is a command the user runs **here** to
/// install it **there** — the recipe the environment page shows for that host,
/// composed in one place so the two can never tell the user different things.
pub(crate) fn remote_pnpm_missing_error(
    host: &crate::config::Host,
    target: &crate::ssh::RemoteHost,
    managers: &[String],
) -> String {
    format!(
        "「{}」上没有可用的 pnpm（需要 {} 或更高版本），所以装不了版本。\n\
         在那台机器上装好之后，回到「设置 → 环境」选中该主机点「重新检测」即可继续：\n\n  {}",
        host.display_name,
        crate::toolchain::MIN_PNPM_MAJOR,
        ToolchainRequirements::for_remote(target, managers).pnpm_install_command
    )
}

/// One candidate a remote probe passed over, as a line of the task log. The page
/// renders the reason codes in the user's language; a log line has no such
/// machinery, so the codes become words here.
pub(crate) fn skipped_candidate_note(tool: &str, skipped: &crate::remote::RemoteSkipped) -> String {
    let why = match skipped.reason.as_str() {
        "below-minimum" => match skipped.major {
            Some(major) => format!("版本过低（{}，主版本 {major}）", skipped.version),
            None => format!("版本过低（{}）", skipped.version),
        },
        "unparsable" => format!("版本号读不出来（{}）", skipped.version),
        "unusable" => "不是能执行的文件".to_string(),
        other => other.to_string(),
    };
    format!("那台机器上的 {tool} 候选 {} 被跳过：{why}", skipped.path)
}

/// Records one host's probe conclusion. The binding is the launcher's
/// resolution, not a preference: it is rewritten whenever it changed, and always
/// into the host it is about — never into a slot two machines would share.
fn write_host_binding(state: &AppState, host_id: &str, binding: &crate::config::ToolchainBinding) {
    let mut cfg = state.config.lock().unwrap();
    let Some(host) = cfg.hosts.iter_mut().find(|host| host.id == host_id) else {
        return;
    };
    if binding_matches(&host.toolchain, binding) {
        return;
    }
    host.toolchain = binding.clone();
    if let Err(e) = crate::config::save_config(&state.config_path, &cfg) {
        crate::log_warn!("保存工具链绑定失败: {e}");
    }
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
    let bound = cfg.local_host().and_then(|host| host.toolchain.node.clone());
    match bound {
        Some(bound) if bound.path.is_file() => bound.path,
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
    node_for_spawn_checked_for(state, None).await
}

/// The node executable to spawn DSH with **on one host**, refreshing a stale
/// binding first. 本机 keeps the shape it always had; a remote host resolves
/// through that machine's own probe, so the path the page reports and the path a
/// spawn there would run are the same file.
pub(crate) async fn node_for_spawn_checked_for(
    state: &AppState,
    host_id: Option<&str>,
) -> Result<PathBuf, String> {
    let (host, target) = env_host(state, host_id)?;
    match target {
        EnvHost::Local => {
            let fresh = {
                let (binding, overrides) = local_toolchain(state);
                binding.node_is_fresh(overrides.node.as_deref())
            };
            if fresh {
                let cfg = state.config.lock().unwrap();
                return Ok(node_for_spawn(&cfg));
            }
            match refresh_local_binding(state, false).await.node {
                Some(bound) => Ok(bound.path),
                None => Err(node_missing_error()),
            }
        }
        EnvHost::Remote(remote) => {
            let env = remote_probe(state, &host, &remote, false).await?;
            match binding_of_remote(&env).node {
                Some(bound) => Ok(bound.path),
                None => Err(node_missing_error()),
            }
        }
    }
}

/// The status the setup page renders for one host — 本机 when the caller names
/// none, which is what every caller written before hosts existed meant.
///
/// `force` re-reads the system environment and re-probes both tools: it is the
/// page's re-check, and the user asking for one means the environment may have
/// moved (a path they just exported, a tool they just installed).
#[tauri::command]
pub async fn get_runtime_status(
    state: State<'_, AppState>,
    force: Option<bool>,
    host_id: Option<String>,
) -> Result<RuntimeStatus, String> {
    runtime_status_for(&state, host_id.as_deref(), force.unwrap_or(false)).await
}

/// Points the launcher at a `node` / `pnpm` the user picked by hand **on one
/// host**, or clears that choice when `path` is absent or empty. Answers with
/// that host's status as it stands afterwards, so one call applies the change
/// and refreshes the page.
///
/// Nothing is written until the file has been run successfully — locally by
/// executing it, on a remote host by running `--version` there: an override that
/// cannot be executed would leave the launcher with a toolchain it cannot spawn,
/// and the user with no obvious way back.
#[tauri::command]
pub async fn set_toolchain_override(
    state: State<'_, AppState>,
    tool: String,
    path: Option<String>,
    host_id: Option<String>,
) -> Result<RuntimeStatus, String> {
    set_toolchain_override_for(&state, &tool, path.as_deref(), host_id.as_deref()).await
}

/// [`set_toolchain_override`] for callers holding `&AppState` — which is every
/// test, since a Tauri `State` only exists inside a running app.
pub(crate) async fn set_toolchain_override_for(
    state: &AppState,
    tool: &str,
    path: Option<&str>,
    host_id: Option<&str>,
) -> Result<RuntimeStatus, String> {
    let (host, target) = env_host(state, host_id)?;
    let program = match tool.trim() {
        "node" => "node",
        "pnpm" => "pnpm",
        other => return Err(format!("无法指定「{other}」：这里只能指定 node 或 pnpm。")),
    };
    let manual = match path.map(str::trim).filter(|raw| !raw.is_empty()) {
        Some(raw) => Some(match &target {
            EnvHost::Local => validate_manual_tool(program, raw).await?,
            EnvHost::Remote(remote) => {
                validate_remote_manual_tool(&*state.host_transport, remote, program, raw).await?
            }
        }),
        None => None,
    };

    {
        let mut cfg = state.config.lock().unwrap();
        let Some(record) = cfg.hosts.iter_mut().find(|record| record.id == host.id) else {
            return Err(format!("主机不存在: {}", host.id));
        };
        let is_node = program == "node";
        let slot = if is_node {
            &mut record.toolchain_overrides.node
        } else {
            &mut record.toolchain_overrides.pnpm
        };
        let changed = match (slot.as_deref(), manual.as_deref()) {
            // Path equality follows the host's own filesystem: this Mac folds
            // case, a Linux machine does not.
            (Some(a), Some(b)) => !host.paths_equal(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            *slot = manual;
            // The recorded binding is the answer to the previous choice; drop it
            // so the refresh below resolves again instead of reusing it.
            if is_node {
                record.toolchain.node = None;
            } else {
                record.toolchain.pnpm = None;
            }
        }
        crate::config::save_config(&state.config_path, &cfg)
            .map_err(|e| format!("保存设置失败: {e}"))?;
    }

    // The cached probe answer is the conclusion of the previous choice too.
    forget_host_env(&host.id);
    runtime_status_for(state, Some(&host.id), false).await
}

/// Probes one host and reports where each tool came from. The one path both the
/// page and the binding go through, so the numbers shown are the ones the
/// launcher will actually spawn there.
pub(crate) async fn runtime_status_for(
    state: &AppState,
    host_id: Option<&str>,
    force: bool,
) -> Result<RuntimeStatus, String> {
    let (host, target) = env_host(state, host_id)?;
    match target {
        EnvHost::Local => Ok(local_runtime_status(state, force).await),
        EnvHost::Remote(remote) => remote_runtime_status(state, &host, &remote, force).await,
    }
}

async fn local_runtime_status(state: &AppState, force: bool) -> RuntimeStatus {
    if force {
        // Warm the cache first: every probe below reads it.
        system_env(true).await;
    }
    // One probe path for both the UI and the binding: the numbers the setup
    // page shows are the ones the launcher will actually spawn.
    let binding = refresh_local_binding(state, force).await;
    let overrides = local_toolchain(state).1;
    let env = system_env(false).await;
    let node = status_from("node", &binding.node, overrides.node.as_deref(), &env.dirs);
    let pnpm = status_from("pnpm", &binding.pnpm, overrides.pnpm.as_deref(), &env.dirs);
    let managers = detected_managers();
    RuntimeStatus {
        host_id: crate::config::LOCAL_HOST_ID.to_string(),
        node_below_recommended: node_is_below_recommended(&node),
        node,
        pnpm,
        requirements: ToolchainRequirements::current(&managers),
        managers,
        system_env_shell: env.shell,
        // The local host passes nothing over: an old Node is reported as a
        // recommendation and still used (ADR-0005 追加 2).
        skipped_candidates: Vec::new(),
    }
}

/// One remote host, as its own interactive login shell answered: the same three
/// sources and the same candidate order as the local host, plus the remote-only
/// rule that a Node below the recommended major is passed over.
async fn remote_runtime_status(
    state: &AppState,
    host: &crate::config::Host,
    target: &crate::ssh::RemoteHost,
    force: bool,
) -> Result<RuntimeStatus, String> {
    // The probe's conclusion *is* the binding: what the page reports is what the
    // launcher will execute there.
    let env = remote_probe(state, host, target, force).await?;

    let node = remote_status(&env.node, host.toolchain_overrides.node.as_deref());
    let pnpm = remote_status(&env.pnpm, host.toolchain_overrides.pnpm.as_deref());
    let mut passed_over = skipped_candidates("node", &env.node);
    passed_over.extend(skipped_candidates("pnpm", &env.pnpm));
    let managers = env.managers.clone();
    Ok(RuntimeStatus {
        host_id: host.id.clone(),
        node_below_recommended: node_is_below_recommended(&node),
        node,
        pnpm,
        requirements: ToolchainRequirements::for_remote(target, &managers),
        managers,
        system_env_shell: Some(env.shell.clone()),
        skipped_candidates: passed_over,
    })
}

/// One remote tool as the page reports it: where it is, what it says, and which
/// source it came from. The user's own choice is carried back even when it was
/// passed over, so the page can say the pick has stopped working.
fn remote_status(
    tool: &crate::remote::RemoteToolStatus,
    manual: Option<&Path>,
) -> ToolStatus {
    let manual_path = manual.map(|p| p.to_string_lossy().into_owned());
    match (tool.path.as_deref(), tool.version.as_deref()) {
        (Some(path), Some(version)) => ToolStatus {
            installed: true,
            version: Some(version.to_string()),
            path: Some(path.to_string()),
            source: Some(match tool.source.as_deref() {
                Some("manual") => ToolSource::Manual,
                Some("system") => ToolSource::System,
                _ => ToolSource::Fallback,
            }),
            manual_path,
        },
        _ => ToolStatus {
            installed: false,
            version: None,
            path: None,
            source: None,
            manual_path,
        },
    }
}

/// The candidates the remote probe passed over on its way to one tool.
fn skipped_candidates(tool: &str, status: &crate::remote::RemoteToolStatus) -> Vec<SkippedCandidate> {
    status
        .skipped
        .iter()
        .map(|skipped| SkippedCandidate {
            tool: tool.to_string(),
            path: skipped.path.clone(),
            version: skipped.version.clone(),
            reason: skipped.reason.clone(),
            major: skipped.major,
        })
        .collect()
}

/// Accepts a hand-typed executable path, or explains in the user's terms why it
/// cannot be used. The file must exist *and* answer `--version` from
/// [`PROBE_CWD`] — the same probe every automatic candidate goes through — so an
/// override the launcher would then fail to spawn is refused up front.
async fn validate_manual_tool(program: &str, raw: &str) -> Result<PathBuf, String> {
    // The app's cwd is wherever Finder launched it from; storing a relative
    // path would mean something different for every later spawn.
    let path = std::path::absolute(raw).map_err(|e| format!("无法解析路径「{raw}」：{e}"))?;
    if !path.is_file() {
        return Err(format!(
            "这个文件不是可用的 {program}：{} 不存在或不是可执行文件。请选择 {program} 可执行文件本身。",
            path.display()
        ));
    }
    let Some(version) = probe_version(&path).await else {
        return Err(format!(
            "这个文件不是可用的 {program}：{} 无法运行（执行 --version 失败）。请选择 {program} 可执行文件本身。",
            path.display()
        ));
    };
    if program == "pnpm" {
        match crate::toolchain::pnpm_major(&version) {
            Some(major) if crate::toolchain::accepts_pnpm_major(major) => {}
            Some(major) => {
                return Err(format!(
                    "这个 pnpm 版本过低：{} 报告的是 pnpm {major}，需要 {} 或更高。\
                     请在终端升级（如 {}）后重新指定。",
                    path.display(),
                    crate::toolchain::MIN_PNPM_MAJOR,
                    crate::toolchain::PNPM_INSTALL_HINT
                ))
            }
            None => {
                return Err(format!(
                    "这个文件不是可用的 pnpm：{} 报告的版本号「{version}」无法识别。",
                    path.display()
                ))
            }
        }
    }
    Ok(path)
}

/// Accepts a hand-typed path **on a remote host**, or explains in the user's
/// terms why it cannot be used.
///
/// The only honest validation is running `--version` on that machine: the
/// launcher cannot execute a file on another computer, and a path that merely
/// looks right here says nothing about there (ADR-0005 追加 3). The version
/// floors are the same ones the local path enforces.
async fn validate_remote_manual_tool(
    transport: &dyn crate::ssh::HostTransport,
    host: &crate::ssh::RemoteHost,
    program: &str,
    raw: &str,
) -> Result<PathBuf, String> {
    let path = raw.trim();
    if !path.starts_with('/') {
        return Err(format!(
            "远端路径要从 / 写起：{path}。远端没有本机文件选择器，请填那台机器上的绝对路径。"
        ));
    }
    match crate::remote::probe_manual_tool(transport, host, path).await {
        Err(err) => Err(err.message),
        Ok(crate::remote::ManualProbe::Missing) => Err(format!(
            "这个文件不是可用的 {program}：远端 {path} 不存在。请确认这是那台机器上的路径，\
             并且指向 {program} 可执行文件本身。"
        )),
        Ok(crate::remote::ManualProbe::Unusable) => Err(format!(
            "这个文件不是可用的 {program}：远端 {path} 无法运行（在那台机器上执行 --version 失败）。\
             请确认它是那台机器上能执行的 {program}。"
        )),
        Ok(crate::remote::ManualProbe::Version(version)) => {
            if program == "pnpm" {
                match crate::toolchain::pnpm_major(&version) {
                    Some(major) if crate::toolchain::accepts_pnpm_major(major) => {}
                    Some(major) => {
                        return Err(format!(
                            "这个 pnpm 版本过低：远端 {path} 报告的是 pnpm {major}，需要 {} 或更高。\
                             请在那台机器上升级（如 {}）后重新指定。",
                            crate::toolchain::MIN_PNPM_MAJOR,
                            crate::remote::ssh_command(
                                &host.target,
                                crate::remote::pnpm_install_recipe()
                            )
                        ))
                    }
                    None => {
                        return Err(format!(
                            "这个文件不是可用的 pnpm：远端 {path} 报告的版本号「{version}」无法识别。"
                        ))
                    }
                }
            }
            Ok(PathBuf::from(path))
        }
    }
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

/// The status of one tool: what the binding resolved to, tagged with the source
/// that produced it and with the user's own choice (which the page reports even
/// when the launcher fell back to something else).
fn status_from(
    program: &str,
    tool: &Option<crate::config::BoundTool>,
    manual: Option<&Path>,
    system_dirs: &[PathBuf],
) -> ToolStatus {
    let manual_path = manual.map(|p| p.to_string_lossy().into_owned());
    match tool {
        Some(bound) => ToolStatus {
            installed: true,
            version: Some(bound.version.clone()),
            path: Some(bound.path.to_string_lossy().into_owned()),
            source: Some(source_of(program, &bound.path, manual, system_dirs)),
            manual_path,
        },
        None => ToolStatus {
            installed: false,
            version: None,
            path: None,
            source: None,
            manual_path,
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
    // Before this function touches PATH: from here on, the process PATH is a
    // widened one, and the *inherited* PATH (what the app was launched with) is
    // only available from the snapshot.
    capture_inherited_path();
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
            source: None,
            manual_path: None,
        };
        // Other majors are supported; 20 is the recommendation floor.
        assert!(node_is_below_recommended(&with(Some("v18.19.0"))));
        assert!(!node_is_below_recommended(&with(Some("v20.0.0"))));
        assert!(!node_is_below_recommended(&with(Some("v24.16.0"))));
        // A missing or unparsable version must not produce a warning.
        assert!(!node_is_below_recommended(&with(None)));
        assert!(!node_is_below_recommended(&with(Some("weird"))));
    }

    /// Somewhere no real tool directory can be: a temp dir, so a test about
    /// ordering does not accidentally pick up the machine's own node.
    fn tmp_bin_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn candidates_put_the_hand_picked_tool_first() {
        let manual_dir = tmp_bin_dir("manual-bin");
        let system_dir = tmp_bin_dir("system-bin");
        let manual = manual_dir.join("node");
        std::fs::write(&manual, "#!/bin/sh\n").unwrap();

        let list = candidates("node", Some(&manual), std::slice::from_ref(&system_dir));
        assert_eq!(list[0], (manual.clone(), ToolSource::Manual), "the user's pick decides");
        assert_eq!(
            list[1],
            (system_dir.join("node"), ToolSource::System),
            "the system environment comes next"
        );
        // Whatever the launcher knows on its own is a fallback only.
        assert!(list.iter().skip(2).all(|(_, s)| *s == ToolSource::Fallback));

        std::fs::remove_dir_all(&manual_dir).ok();
        std::fs::remove_dir_all(&system_dir).ok();
    }

    #[test]
    fn candidates_list_each_path_once_and_keep_the_first_source() {
        // `~/.local/bin` is also one of the launcher's own fallback directories,
        // so a shell PATH that names it must still read as the system's — the
        // first source to meet a path is the one that decides. The same holds
        // for a directory the PATH names twice.
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let dir = home.join(".local").join("bin");
        for dirs in [vec![dir.clone()], vec![dir.clone(), dir.clone()]] {
            let list = candidates("node", None, &dirs);
            let exe = dir.join("node");
            assert_eq!(
                list.iter().filter(|(p, _)| p == &exe).count(),
                1,
                "a path must be listed once: {list:?}"
            );
            assert_eq!(source_of("node", &exe, None, &dirs), ToolSource::System);
        }
    }

    /// fnm hands each shell its own directory of hard links into the version it
    /// selected. Both spellings name one file; the durable one is what gets
    /// bound, and the source still says the terminal has it.
    #[test]
    fn candidates_bind_the_durable_spelling_of_the_same_file() {
        let shell_dir = tmp_bin_dir("fnm-multishell");
        let durable_dir = tmp_bin_dir("fnm-version");
        let in_shell = shell_dir.join("node");
        std::fs::write(&in_shell, "#!/bin/sh\n").unwrap();
        std::fs::hard_link(&in_shell, durable_dir.join("node")).unwrap();

        let list = candidates_with(
            "node",
            None,
            std::slice::from_ref(&shell_dir),
            std::slice::from_ref(&durable_dir),
        );
        assert_eq!(
            list,
            vec![(durable_dir.join("node"), ToolSource::System)],
        );

        std::fs::remove_dir_all(&shell_dir).ok();
        std::fs::remove_dir_all(&durable_dir).ok();
    }

    /// A copy is a different file: the directory the shell named keeps the
    /// binding, and the launcher's own copy stays a fallback for the day the
    /// first one is gone.
    #[test]
    fn candidates_keep_a_copy_where_the_system_named_it() {
        let shell_dir = tmp_bin_dir("copy-shell");
        let fallback_dir = tmp_bin_dir("copy-fallback");
        let in_shell = shell_dir.join("node");
        std::fs::write(&in_shell, "#!/bin/sh\n").unwrap();
        std::fs::copy(&in_shell, fallback_dir.join("node")).unwrap();

        let list = candidates_with(
            "node",
            None,
            std::slice::from_ref(&shell_dir),
            std::slice::from_ref(&fallback_dir),
        );
        assert_eq!(
            list,
            vec![
                (shell_dir.join("node"), ToolSource::System),
                (fallback_dir.join("node"), ToolSource::Fallback),
            ]
        );

        std::fs::remove_dir_all(&shell_dir).ok();
        std::fs::remove_dir_all(&fallback_dir).ok();
    }

    /// A recorded path can be spelled differently from today's list while naming
    /// the same tool, so the answer follows the file.
    #[test]
    fn a_path_matches_a_candidate_by_file_not_by_spelling() {
        let picked_dir = tmp_bin_dir("picked");
        let alias_dir = tmp_bin_dir("picked-alias");
        let picked = picked_dir.join("node");
        std::fs::write(&picked, "#!/bin/sh\n").unwrap();
        let alias = alias_dir.join("node");
        std::fs::hard_link(&picked, &alias).unwrap();

        assert_eq!(
            source_of("node", &alias, Some(&picked), &[]),
            ToolSource::Manual,
            "the same file as the pick is still the pick"
        );
        assert_eq!(
            source_of("node", Path::new("/nowhere/bin/node"), Some(&picked), &[]),
            ToolSource::Fallback,
            "a file nothing in the list names is the launcher's own find"
        );

        std::fs::remove_dir_all(&picked_dir).ok();
        std::fs::remove_dir_all(&alias_dir).ok();
    }

    #[test]
    fn source_is_reported_from_the_candidate_order() {
        let manual_dir = tmp_bin_dir("manual-src");
        let system_dir = tmp_bin_dir("system-src");
        let manual = manual_dir.join("node");
        std::fs::write(&manual, "#!/bin/sh\n").unwrap();
        let system_dirs = vec![system_dir.clone()];

        assert_eq!(
            source_of("node", &manual, Some(&manual), &system_dirs),
            ToolSource::Manual
        );
        assert_eq!(
            source_of("node", &system_dir.join("node"), Some(&manual), &system_dirs),
            ToolSource::System
        );
        // A path outside both the pick and the system environment — including
        // one the launcher would not itself offer — reads as a fallback rather
        // than making the page claim a source it cannot prove.
        assert_eq!(
            source_of("node", Path::new("/nowhere/bin/node"), Some(&manual), &system_dirs),
            ToolSource::Fallback
        );

        std::fs::remove_dir_all(&manual_dir).ok();
        std::fs::remove_dir_all(&system_dir).ok();
    }

    #[test]
    fn a_hand_picked_path_outranks_the_same_directory_entry() {
        // Both the pick and the system environment name the very same file: the
        // user's deliberate choice is what the page must say.
        let dir = tmp_bin_dir("same-bin");
        let exe = dir.join("node");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        assert_eq!(
            candidates("node", Some(&exe), std::slice::from_ref(&dir))[0],
            (exe.clone(), ToolSource::Manual)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_shell_path_survives_a_noisy_login_shell() {
        // Plugin managers print their own banners while `.zshrc` runs, so only
        // what sits between the markers may be taken as PATH.
        assert_eq!(
            parse_shell_path("compinit: 3 warnings\n<<<DSH-PATH>>>/usr/bin:/bin<<<DSH-END>>>\n"),
            Some("/usr/bin:/bin".to_string())
        );
        assert_eq!(
            parse_shell_path("<<<DSH-PATH>>>  /opt/bin:/usr/bin  <<<DSH-END>>>"),
            Some("/opt/bin:/usr/bin".to_string()),
            "surrounding whitespace is not part of a PATH"
        );
        assert_eq!(
            parse_shell_path("<<<DSH-PATH>>>/a:/b<<<DSH-END>>>trailing noise"),
            Some("/a:/b".to_string())
        );
    }

    #[test]
    fn parse_shell_path_rejects_output_without_the_markers() {
        // A shell that failed, or one that overrides `printf`, gives nothing
        // usable — the caller then falls back to the inherited PATH.
        assert_eq!(parse_shell_path("/usr/bin:/bin\n"), None);
        assert_eq!(parse_shell_path("<<<DSH-PATH>>>/usr/bin:/bin"), None);
        assert_eq!(parse_shell_path(""), None);
        assert_eq!(parse_shell_path("<<<DSH-PATH>>><<<DSH-END>>>"), None);
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

    // -----------------------------------------------------------------------
    // Per host
    // -----------------------------------------------------------------------

    /// A scratch directory that also holds the config, so a probe's conclusion
    /// can be checked where it is persisted.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn app_state(
        data: &Path,
        hosts: Vec<crate::config::Host>,
        transport: std::sync::Arc<dyn crate::ssh::HostTransport>,
    ) -> AppState {
        AppState {
            config_path: data.join("config.json"),
            data_dir: data.to_path_buf(),
            config: std::sync::Mutex::new(crate::config::Config {
                hosts,
                ..Default::default()
            }),
            running: Default::default(),
            tasks: Default::default(),
            host_transport: transport,
            host_statuses: Default::default(),
            remote_scan: std::sync::Mutex::new(std::collections::HashSet::new()),
            forwards: Default::default(),
        }
    }

    fn remote_host(id: &str) -> crate::config::Host {
        crate::config::Host {
            id: id.to_string(),
            display_name: "服务器".into(),
            ssh_target: "lab".into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: Some(PathBuf::from("/home/demo/.local/share/dsh-launcher")),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    /// One host's record as a restart would read it back from disk.
    fn persisted(state: &AppState, id: &str) -> crate::config::Host {
        let cfg = crate::config::load_config(&state.config_path);
        cfg.hosts
            .iter()
            .find(|host| host.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("config.json 里没有 {id}"))
    }

    /// Whether a refused write left anything on disk: no config file at all, or
    /// a record with neither tool picked by hand.
    fn nothing_written(state: &AppState, id: &str) -> bool {
        if !state.config_path.exists() {
            return true;
        }
        crate::config::load_config(&state.config_path)
            .hosts
            .iter()
            .find(|host| host.id == id)
            .map(|host| {
                host.toolchain_overrides.node.is_none() && host.toolchain_overrides.pnpm.is_none()
            })
            .unwrap_or(true)
    }

    /// A real executable the local probe can run, printing the version it was
    /// made with.
    fn fake_exe(dir: &Path, name: &str, version: &str) -> PathBuf {
        let exe = dir.join(name);
        std::fs::write(&exe, format!("#!/bin/sh\necho {version}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        exe
    }

    /// The local host is the user's own desktop: it says an old Node is old and
    /// keeps using it. The remote-only rule (pass it over) must not leak here.
    #[tokio::test]
    async fn the_local_host_warns_about_an_old_node_instead_of_skipping_it() {
        let data = Scratch::new("runtime-local-old");
        let old = fake_exe(data.path(), "node", "v18.19.1");
        let pnpm = fake_exe(data.path(), "pnpm", "11.17.0");
        let state = app_state(
            data.path(),
            vec![crate::config::Host::local()],
            crate::remote::testing::idle(),
        );
        set_toolchain_override_for(&state, "node", Some(old.to_str().unwrap()), None)
            .await
            .unwrap();
        set_toolchain_override_for(&state, "pnpm", Some(pnpm.to_str().unwrap()), None)
            .await
            .unwrap();

        let status = runtime_status_for(&state, None, false).await.unwrap();

        assert_eq!(status.host_id, crate::config::LOCAL_HOST_ID);
        assert!(status.node.installed, "老 Node 仍然可用，只是要提醒");
        assert_eq!(status.node.path.as_deref(), Some(old.to_str().unwrap()));
        assert_eq!(status.node.source, Some(ToolSource::Manual));
        assert!(status.node_below_recommended, "低版本要如实提示");
        assert!(
            status.skipped_candidates.is_empty(),
            "本机不跳过任何候选：{:?}",
            status.skipped_candidates
        );
    }

    /// The ticket's core promise: two machines, two conclusions, two picks. The
    /// probe cache and the user's choice are keyed by host, and neither host's
    /// answer can be written into the other's record.
    #[tokio::test]
    async fn each_host_keeps_its_own_conclusion_and_its_own_pick() {
        let data = Scratch::new("runtime-per-host");
        let remote_id = format!("host-lab-{}", uuid::Uuid::new_v4());
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::marked(r#"{"exists":true,"version":"v22.14.0"}"#),
            ))
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::environment_stdout(
                    crate::remote::testing::remote_tool("/opt/pick/node", "v22.14.0", "manual"),
                    crate::remote::testing::remote_tool("/usr/local/bin/pnpm", "11.17.0", "system"),
                    &[],
                ),
            ));
        let state = app_state(
            data.path(),
            vec![crate::config::Host::local(), remote_host(&remote_id)],
            fake,
        );
        let local_node = fake_exe(data.path(), "local-node", "v24.0.0");

        let remote = set_toolchain_override_for(
            &state,
            "node",
            Some("/opt/pick/node"),
            Some(&remote_id),
        )
        .await
        .unwrap();
        let local = set_toolchain_override_for(&state, "node", Some(local_node.to_str().unwrap()), None)
            .await
            .unwrap();

        assert_eq!(remote.node.path.as_deref(), Some("/opt/pick/node"));
        assert_eq!(remote.node.source, Some(ToolSource::Manual));
        assert_eq!(local.node.path.as_deref(), Some(local_node.to_str().unwrap()));
        assert_eq!(local.node.source, Some(ToolSource::Manual));

        // Each record keeps its own pair, on disk as well as in memory.
        let local_record = persisted(&state, crate::config::LOCAL_HOST_ID);
        let remote_record = persisted(&state, &remote_id);
        assert_eq!(
            local_record.toolchain_overrides.node.as_deref(),
            Some(local_node.as_path())
        );
        assert_eq!(
            remote_record.toolchain_overrides.node.as_deref(),
            Some(Path::new("/opt/pick/node"))
        );
        assert!(
            local_record.toolchain_overrides.pnpm.is_none() && remote_record.toolchain_overrides.pnpm.is_none(),
            "指定 node 不该动 pnpm"
        );
    }

    /// Two reachable machines are two answers. A single PATH cache would report
    /// whichever host was probed last (ADR-0005 追加).
    #[tokio::test]
    async fn two_machines_do_not_share_one_probe_answer() {
        let data = Scratch::new("runtime-two-machines");
        let first_id = format!("host-a-{}", uuid::Uuid::new_v4());
        let second_id = format!("host-b-{}", uuid::Uuid::new_v4());
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::environment_stdout(
                    crate::remote::testing::remote_tool("/home/a/.nvm/bin/node", "v22.1.0", "system"),
                    crate::remote::testing::remote_tool_missing(vec![]),
                    &["nvm"],
                ),
            ))
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::environment_stdout(
                    crate::remote::testing::remote_tool(
                        "/home/linuxbrew/.linuxbrew/bin/node",
                        "v26.0.0",
                        "fallback",
                    ),
                    crate::remote::testing::remote_tool_missing(vec![]),
                    &["brew"],
                ),
            ));
        let state = app_state(
            data.path(),
            vec![
                crate::config::Host::local(),
                remote_host(&first_id),
                remote_host(&second_id),
            ],
            fake,
        );

        let first = runtime_status_for(&state, Some(&first_id), false).await.unwrap();
        let second = runtime_status_for(&state, Some(&second_id), false).await.unwrap();

        assert_eq!(first.host_id, first_id);
        assert_eq!(second.host_id, second_id);
        assert_eq!(first.node.path.as_deref(), Some("/home/a/.nvm/bin/node"));
        assert_eq!(first.node.source, Some(ToolSource::System));
        assert_eq!(first.managers, vec!["nvm".to_string()]);
        assert_eq!(
            second.node.path.as_deref(),
            Some("/home/linuxbrew/.linuxbrew/bin/node"),
            "第二台机器的结论不能被第一台的缓存盖掉"
        );
        assert_eq!(second.node.source, Some(ToolSource::Fallback));
        assert_eq!(second.managers, vec!["brew".to_string()]);
    }

    /// A remote conclusion travels with its source, with what was passed over,
    /// and with commands the user can actually run from this Mac.
    #[tokio::test]
    async fn a_remote_probe_reports_its_source_and_what_it_passed_over() {
        let data = Scratch::new("runtime-remote-source");
        let remote_id = format!("host-src-{}", uuid::Uuid::new_v4());
        let old = "/usr/bin/node";
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::environment_stdout(
                {
                    let mut tool = crate::remote::testing::remote_tool(
                        "/home/demo/.nvm/versions/node/v22.14.0/bin/node",
                        "v22.14.0",
                        "system",
                    );
                    tool["skipped"] = serde_json::json!([crate::remote::testing::skipped(
                        old,
                        "v18.19.1",
                        "below-minimum",
                        Some(18)
                    )]);
                    tool
                },
                crate::remote::testing::remote_tool_missing(vec![crate::remote::testing::skipped(
                    "/usr/bin/pnpm",
                    "9.15.0",
                    "below-minimum",
                    Some(9),
                )]),
                &["nvm"],
            )),
        );
        let state = app_state(
            data.path(),
            vec![crate::config::Host::local(), remote_host(&remote_id)],
            fake,
        );

        let status = runtime_status_for(&state, Some(&remote_id), false).await.unwrap();

        assert_eq!(status.node.source, Some(ToolSource::System));
        assert_eq!(status.system_env_shell.as_deref(), Some("/bin/bash"));
        assert_eq!(status.managers, vec!["nvm".to_string()]);
        assert!(!status.pnpm.installed);

        let skipped: Vec<&SkippedCandidate> = status
            .skipped_candidates
            .iter()
            .filter(|candidate| candidate.reason == "below-minimum")
            .collect();
        assert_eq!(skipped.len(), 2, "两个候选都要如实报出来：{:?}", status.skipped_candidates);
        assert_eq!(skipped[0].tool, "node");
        assert_eq!(skipped[0].path, old);
        assert_eq!(skipped[0].major, Some(18));
        assert_eq!(skipped[1].tool, "pnpm");
        assert_eq!(skipped[1].major, Some(9));

        // The install guidance is one command the user runs here, over ssh, in
        // the same shell shape the probe used.
        assert!(
            status
                .requirements
                .pnpm_install_command
                .starts_with("ssh lab "),
            "{}",
            status.requirements.pnpm_install_command
        );
        assert!(
            status.requirements.pnpm_install_command.contains("get.pnpm.io"),
            "{}",
            status.requirements.pnpm_install_command
        );
        assert!(status.requirements.node_install_command.contains("nvm install --lts"));
    }

    /// Probing and execution are the same resolution on a remote host too: the
    /// path a spawn would use is the path the page reported — and a second
    /// question inside the cache window does not cost another ssh call.
    #[tokio::test]
    async fn a_remote_probe_and_a_remote_spawn_resolve_to_the_same_candidate() {
        let data = Scratch::new("runtime-remote-agree");
        let remote_id = format!("host-agree-{}", uuid::Uuid::new_v4());
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::environment_stdout(
                crate::remote::testing::remote_tool(
                    "/home/linuxbrew/.linuxbrew/bin/node",
                    "v26.0.0",
                    "fallback",
                ),
                crate::remote::testing::remote_tool("/usr/local/bin/pnpm", "11.17.0", "system"),
                &["brew"],
            )),
        );
        let state = app_state(
            data.path(),
            vec![crate::config::Host::local(), remote_host(&remote_id)],
            fake.clone(),
        );

        let status = runtime_status_for(&state, Some(&remote_id), false).await.unwrap();
        let to_spawn = node_for_spawn_checked_for(&state, Some(&remote_id))
            .await
            .unwrap();

        assert_eq!(status.node.path.as_deref(), Some(to_spawn.to_str().unwrap()));
        assert_eq!(
            fake.calls().len(),
            1,
            "同一个窗口内第二次提问不该再起一次 shell：{:?}",
            fake.calls()
        );
        // And the record a restart would read agrees too.
        let record = persisted(&state, &remote_id);
        assert_eq!(
            record.toolchain.node.as_ref().map(|bound| bound.path.clone()),
            Some(to_spawn)
        );
    }

    /// A remote pick is validated by running it **there**. A path the launcher
    /// could not run is refused and nothing is written — the same call the local
    /// host makes, for the same reason.
    #[tokio::test]
    async fn a_remote_pick_is_refused_when_it_cannot_be_run_there() {
        let data = Scratch::new("runtime-remote-pick");
        let remote_id = format!("host-pick-{}", uuid::Uuid::new_v4());
        let fake = crate::remote::testing::FakeTransport::new()
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::marked(r#"{"exists":false,"version":null}"#),
            ))
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::marked(r#"{"exists":true,"version":"9.15.0"}"#),
            ))
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::marked(r#"{"exists":true,"version":"v22.14.0"}"#),
            ))
            .answering(crate::remote::testing::FakeAnswer::stdout(
                crate::remote::testing::environment_stdout(
                    crate::remote::testing::remote_tool("/opt/pick/node", "v22.14.0", "manual"),
                    crate::remote::testing::remote_tool_missing(vec![]),
                    &[],
                ),
            ));
        let state = app_state(
            data.path(),
            vec![crate::config::Host::local(), remote_host(&remote_id)],
            fake.clone(),
        );

        // Not there: the message names the path on *that* machine, and nothing
        // is written.
        let err = set_toolchain_override_for(&state, "node", Some("/gone/node"), Some(&remote_id))
            .await
            .unwrap_err();
        assert!(err.contains("/gone/node") && err.contains("远端"), "{err}");
        assert!(nothing_written(&state, &remote_id), "被拒绝的指定不能落库");

        // There, but a pnpm the launcher will not drive: refused, with the
        // command that fixes it on that machine.
        let err = set_toolchain_override_for(&state, "pnpm", Some("/usr/bin/pnpm"), Some(&remote_id))
            .await
            .unwrap_err();
        assert!(err.contains("pnpm 9") && err.contains("ssh lab "), "{err}");
        assert!(nothing_written(&state, &remote_id), "被拒绝的指定不能落库");

        // A path that runs: accepted, and reported back as the manual source.
        let status =
            set_toolchain_override_for(&state, "node", Some("/opt/pick/node"), Some(&remote_id))
                .await
                .unwrap();
        assert_eq!(status.node.source, Some(ToolSource::Manual));
        assert_eq!(
            persisted(&state, &remote_id)
                .toolchain_overrides
                .node
                .as_deref(),
            Some(Path::new("/opt/pick/node"))
        );
    }

    /// A pick that stopped working falls back to detection but is still reported,
    /// so the page can say so and let the user replace or clear it (ADR-0005).
    #[tokio::test]
    async fn a_remote_pick_that_stopped_working_falls_back_and_is_still_reported() {
        let data = Scratch::new("runtime-remote-stale");
        let remote_id = format!("host-stale-{}", uuid::Uuid::new_v4());
        let mut host = remote_host(&remote_id);
        host.toolchain_overrides.node = Some(PathBuf::from("/gone/node"));
        let fake = crate::remote::testing::FakeTransport::new().answering(
            crate::remote::testing::FakeAnswer::stdout(crate::remote::testing::environment_stdout(
                {
                    let mut tool = crate::remote::testing::remote_tool(
                        "/usr/local/bin/node",
                        "v22.14.0",
                        "system",
                    );
                    tool["skipped"] = serde_json::json!([crate::remote::testing::skipped(
                        "/gone/node",
                        "",
                        "unusable",
                        None
                    )]);
                    tool
                },
                crate::remote::testing::remote_tool_missing(vec![]),
                &[],
            )),
        );
        let state = app_state(
            data.path(),
            vec![crate::config::Host::local(), host],
            fake,
        );

        let status = runtime_status_for(&state, Some(&remote_id), false).await.unwrap();

        assert!(status.node.installed, "退回自动探测，工具本身还是可用的");
        assert_eq!(status.node.path.as_deref(), Some("/usr/local/bin/node"));
        assert_eq!(status.node.source, Some(ToolSource::System));
        assert_eq!(
            status.node.manual_path.as_deref(),
            Some("/gone/node"),
            "失效的指定要如实带回，页面才好说「你指定的路径用不了了」"
        );
        assert_eq!(status.skipped_candidates[0].reason, "unusable");
    }
}
