//! Every remote script the launcher runs, and the reading of their answers.
//!
//! Two rules hold for the whole remote feature (issue #9, 「传输层」):
//!
//! * **The shell text lives here.** A call site asks this module for a script
//!   and hands the result to the transport; nobody assembles shell strings of
//!   their own, because everything that can go wrong with quoting goes wrong
//!   there.
//! * **An answer comes back as JSON between two markers.** A Linux login shell
//!   prints things of its own (`~/.bashrc` banners, version notices, and
//!   `bash -i`'s 「no job control」), so the whole of stdout is not usable as the
//!   answer — only what sits between `<<<DSH-JSON>>>` and `<<<DSH-END>>>` is.
//!   `runtime::parse_shell_path` reads the local login shell's `PATH` the same
//!   way.
//!
//! Probing here is deliberately raw: the connection test reports what a machine
//! resolves `node` and `pnpm` to, and nothing more. The three sources
//! (系统环境变量 / 常见安装目录 / 用户指定) and the 「远端跳过 Node 低于 20 的候选」 rule
//! belong to the remote environment page, which extends `dsh_tool` below rather
//! than re-deriving it (ADR-0005 追加).

use crate::ssh::{
    HostError, HostErrorKind, HostTransport, RemoteCall, RemoteHost, RemoteOutput,
    DEFAULT_TIMEOUT,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Marks the answer. Read back between these, never around them.
pub const MARK: &str = "<<<DSH-JSON>>>";
pub const END: &str = "<<<DSH-END>>>";

/// Marks the line a [`measured_argv`] wrapper appends to its command's log: the
/// exit status. A detached process is nobody's child, so nothing can `wait` for
/// it — this line written next to its own output is the only place its outcome
/// exists (see [`measured_argv`]).
pub const EXIT_MARK: &str = "<<<DSH-EXIT>>>";

/// POSIX single-quoting: the one escaping rule every remote script uses, and
/// the reason a path with a space or a quote in it survives.
///
/// A remote word goes through **two** shells — the local ssh argv, which joins
/// the command words with spaces, and the remote login shell, which splits them
/// again. Both are ordinary shells, so one rule covers both.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Reads the marked JSON block out of a script's stdout.
///
/// A missing block and unreadable JSON are both 「远端命令失败」: the remote side
/// did not do what it was asked, and its stderr is the best explanation there
/// is.
pub fn parse_marked<T: DeserializeOwned>(out: &RemoteOutput) -> Result<T, HostError> {
    let payload = between_markers(&out.stdout)
        .ok_or_else(|| failed(out, "远端脚本没有返回结果标记"))?;
    serde_json::from_str(payload)
        .map_err(|e| failed(out, &format!("远端脚本的结果无法解析：{e}")))
}

fn between_markers(stdout: &str) -> Option<&str> {
    let start = stdout.find(MARK)? + MARK.len();
    let rest = &stdout[start..];
    let end = rest.find(END)?;
    Some(rest[..end].trim())
}

fn failed(out: &RemoteOutput, note: &str) -> HostError {
    let said = match crate::ssh::stderr_tail(&out.stderr) {
        tail if tail.is_empty() => "远端没有说明原因".to_string(),
        tail => tail,
    };
    let message = if out.status == 0 {
        format!("{note}：{said}")
    } else {
        format!("远端命令失败（退出码 {}）：{said}", out.status)
    };
    HostError::remote_command(message)
}

/// Runs one script and reads its marked JSON answer.
pub async fn json<T: DeserializeOwned>(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    script: String,
    timeout: Duration,
) -> Result<T, HostError> {
    let out = transport.run(host, RemoteCall::within(script, timeout)).await?;
    parse_marked(&out)
}

/// How long a version listing may take. `npm view` on a mirror that has never
/// answered this machine before is slower than any script whose answer is
/// already local, and it is the one remote call whose result the user waits
/// for standing still.
const NPM_VIEW_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// The connection test
// ---------------------------------------------------------------------------

/// A machine's system, as reported by `uname`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSystem {
    pub os: String,
    pub release: String,
    pub arch: String,
}

/// One probe of one tool on a remote machine: the absolute path the login shell
/// resolves it to, and what `--version` said. `None` means the machine has no
/// such command, which is a fact to report, not an error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTool {
    pub path: String,
    pub version: String,
}

/// What 「测试连接」 found on a host.
///
/// A failure is a **result**, not an error: the user may save a machine that is
/// simply switched off right now, and the page then says 「不可达」 rather than
/// treating the host record as broken (ADR-0006). `kind` carries the class, so
/// the page can tell 连不上 / 认证失败 / 超时 / 远端命令失败 apart.
#[derive(Clone, Debug, Serialize)]
pub struct ConnectionTest {
    pub ok: bool,
    pub kind: Option<HostErrorKind>,
    pub message: Option<String>,
    pub system: Option<RemoteSystem>,
    pub home: Option<String>,
    /// The launcher's data directory *on that machine*, with `~` already
    /// expanded there — the launcher cannot expand a remote path itself.
    pub data_dir: Option<String>,
    /// Whether that directory is already there. A fresh host has none until the
    /// launcher first writes something into it.
    pub data_dir_exists: bool,
    pub node: Option<RemoteTool>,
    pub pnpm: Option<RemoteTool>,
}

impl ConnectionTest {
    fn refused(err: HostError) -> Self {
        Self {
            ok: false,
            kind: Some(err.kind),
            message: Some(err.message),
            system: None,
            home: None,
            data_dir: None,
            data_dir_exists: false,
            node: None,
            pnpm: None,
        }
    }

    fn answered(facts: Facts) -> Self {
        Self {
            ok: true,
            kind: None,
            message: None,
            system: Some(RemoteSystem {
                os: facts.os,
                release: facts.release,
                arch: facts.arch,
            }),
            home: Some(facts.home),
            data_dir: Some(facts.data_dir),
            data_dir_exists: facts.data_dir_exists,
            node: facts.node,
            pnpm: facts.pnpm,
        }
    }
}

/// The script's answer, field for field.
#[derive(Deserialize)]
struct Facts {
    os: String,
    release: String,
    arch: String,
    home: String,
    data_dir: String,
    data_dir_exists: bool,
    node: Option<RemoteTool>,
    pnpm: Option<RemoteTool>,
}

/// Runs the connection test on one host.
///
/// `data_dir` is the host record's configured data directory, or `None` for the
/// default — the script resolves it *there*, because `~` and existence are
/// facts only that machine has.
pub async fn connection_test(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    data_dir: Option<&str>,
) -> ConnectionTest {
    let script = connection_test_script(data_dir);
    match json::<Facts>(transport, host, script, DEFAULT_TIMEOUT).await {
        Ok(facts) => ConnectionTest::answered(facts),
        Err(err) => ConnectionTest::refused(err),
    }
}

/// The versions one remote machine has installed, read from that machine.
///
/// The launcher's records say which versions the user asked for on that host;
/// only the machine can say what is really in its versions directory and
/// whether each version's bin is there — a directory left behind by an install
/// that stopped early is not a version anyone can start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInstalledVersion {
    /// The directory's name under `versions/`, which is the version string.
    pub name: String,
    pub dir: String,
    /// Whether that version's bin exists and is not empty. The layout is the
    /// local launcher's (`launch::version_bin`), applied on that machine.
    pub bin_ready: bool,
}

/// What one machine's `npm view` said about `@deepseek-ai/dsh`: the version
/// list and the publish times, both **as text** — the same two strings the
/// local `npm view` hands over, so one conversion serves both sources.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NpmListings {
    pub versions: String,
    pub time: String,
}

/// The versions installed under `versions_dir`, as the machine describes them.
pub async fn installed_versions(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    versions_dir: &str,
) -> Result<Vec<RemoteInstalledVersion>, HostError> {
    let out = transport
        .run(host, RemoteCall::new(installed_versions_script(versions_dir)))
        .await?;
    decode_installed_versions(&out)
}

/// What `@deepseek-ai/dsh` can be installed from on one machine, asked of that
/// machine's own npm against the registry its host record names.
pub async fn available_versions(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    registry: &str,
) -> Result<NpmListings, HostError> {
    json(
        transport,
        host,
        available_versions_script(registry),
        NPM_VIEW_TIMEOUT,
    )
    .await
}

// ---------------------------------------------------------------------------
// Following one detached command
// ---------------------------------------------------------------------------

/// What one read of a remote log left behind: the text appended since the
/// previous offset, and whether the process tree is still there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailChunk {
    pub text: String,
    /// Where to read from next. Always the log's current size, so the next call
    /// asks only for what was written after this one.
    pub offset: u64,
    pub alive: bool,
}

/// Reads a remote log from `offset`, waiting **on that machine** for output, for
/// the process to go, or for the budget, whichever comes first.
pub async fn tail(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    log_path: &str,
    offset: u64,
    pid: i32,
    wait_secs: u64,
) -> Result<TailChunk, HostError> {
    #[derive(Deserialize)]
    struct Chunk {
        offset: u64,
        data: String,
        alive: bool,
    }
    let script = tail_script(log_path, offset, pid, wait_secs);
    let out = transport.run(host, RemoteCall::within(script, DEFAULT_TIMEOUT)).await?;
    let chunk: Chunk = parse_marked(&out)?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk.data.as_bytes())
        .map_err(|e| HostError::remote_command(format!("远端日志无法解码：{e}")))?;
    Ok(TailChunk {
        text: String::from_utf8_lossy(&bytes).to_string(),
        offset: chunk.offset,
        alive: chunk.alive,
    })
}

/// Whether the process tree one detached spawn started is still there.
pub async fn alive(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    pid: i32,
) -> Result<bool, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        alive: bool,
    }
    let out = transport
        .run(host, RemoteCall::new(alive_script(pid)))
        .await?;
    Ok(parse_marked::<Answer>(&out)?.alive)
}

/// One listener on a port **on that machine**: its pid when `ss` could name it,
/// and the command `ps` reports. A listener belonging to another user comes
/// back without a pid and with an empty command — 「被占着，但说不出是谁」.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePortOwner {
    pub pid: Option<i32>,
    pub command: String,
}

/// Who holds `port` on that machine. `None` = nothing is listening there.
///
/// The probe runs **there**; a local `lsof` would answer about this Mac's
/// listeners (`crate::process::probe_port_owner` is that half).
pub async fn port_owner(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    port: u16,
) -> Result<Option<RemotePortOwner>, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        occupied: bool,
        pid: Option<i32>,
        command: String,
    }
    let out = transport
        .run(host, RemoteCall::new(port_owner_script(port)))
        .await?;
    let answer: Answer = parse_marked(&out)?;
    Ok(answer.occupied.then_some(RemotePortOwner {
        pid: answer.pid,
        command: answer.command,
    }))
}

/// Whether the process tree one detached spawn started is gone, waiting up to
/// `wait_secs` **on that machine** for it to go.
pub async fn wait_gone(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    pid: i32,
    wait_secs: u64,
) -> Result<bool, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        gone: bool,
    }
    let out = transport
        .run(
            host,
            RemoteCall::within(wait_gone_script(pid, wait_secs), DEFAULT_TIMEOUT),
        )
        .await?;
    Ok(parse_marked::<Answer>(&out)?.gone)
}

/// What one attempt to spend an instance URL's launch token left behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteMint {
    /// The HTTP status, 0 when that machine has no `curl` (or the request never
    /// got an answer).
    pub status: u16,
    /// The `Set-Cookie` pair, when the instance sent one.
    pub cookie: Option<String>,
}

/// Asks **that machine** to spend one instance URL's launch token, and reports
/// what its instance answered.
///
/// The token belongs to an instance listening on that machine's loopback, so
/// only a request made there can say whether it is still accepted (ADR-0006:
/// the same URL read from here would reach a different service). Whether the
/// answer counts as 「认证成功」 is [`crate::native`]'s business — the cookie name
/// is bound to the authority — and that decision stays in one place.
pub async fn mint_headers(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    url: &str,
) -> Result<RemoteMint, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        status: u16,
        cookie: Option<String>,
    }
    let out = transport
        .run(host, RemoteCall::new(mint_script(url)))
        .await?;
    let answer: Answer = parse_marked(&out)?;
    Ok(RemoteMint {
        status: answer.status,
        cookie: answer.cookie,
    })
}

/// How many bytes one remote log already holds.
///
/// Read *before* a detached spawn: the log is appended to across runs (a stopped
/// instance's log is not truncated), and the launcher has to follow this run's
/// output only. Starting at zero would re-read the previous run's URL line and
/// adopt a token that belongs to a process which is no longer there.
pub async fn log_size(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    log_path: &str,
) -> Result<u64, HostError> {
    #[derive(Deserialize)]
    struct Size {
        offset: u64,
    }
    let out = transport
        .run(host, RemoteCall::new(log_size_script(log_path)))
        .await?;
    Ok(parse_marked::<Size>(&out)?.offset)
}

/// Whether one file is there and not empty — the local rule a version's `bin.js`
/// is checked with (`-s`), asked of the machine that owns the file.
pub async fn file_ready(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<bool, HostError> {
    #[derive(Deserialize)]
    struct Ready {
        ready: bool,
    }
    let out = transport
        .run(host, RemoteCall::new(file_ready_script(path)))
        .await?;
    Ok(parse_marked::<Ready>(&out)?.ready)
}

/// When one pid started, as that machine's own `/proc/<pid>/stat` records it
/// (field 22: clock ticks since boot). A pid number alone is not an identity —
/// the kernel reuses it — so this is what tells the process the launcher started
/// from a later one wearing the same number (ADR-0007 §3).
///
/// `None` means the pid is not there (or the file cannot be read): the caller
/// decides whether that is "already gone" or "cannot check".
pub async fn process_start_time(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    pid: i32,
) -> Result<Option<u64>, HostError> {
    #[derive(Deserialize)]
    struct Started {
        started_at: Option<u64>,
    }
    let out = transport
        .run(host, RemoteCall::new(start_time_script(pid)))
        .await?;
    Ok(parse_marked::<Started>(&out)?.started_at)
}

/// Reads one file, or answers `None` when that machine says the file is not
/// there.
///
/// [`READ_FILE`] exits 2 for exactly that case, and it is a normal answer for
/// the files the launcher reads before it has ever written them (a host's
/// running record, a profile's patch document). Every other failure — an
/// unreachable machine, unreadable bytes — is still an `Err`.
pub async fn read_file_optional(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<Option<Vec<u8>>, HostError> {
    let out = transport
        .run(host, RemoteCall::new(read_file_script(path)))
        .await?;
    if out.status == READ_FILE_MISSING {
        return Ok(None);
    }
    decode_blob(&out).map(Some)
}

/// The exit status [`READ_FILE`] uses for 「这台机器上没有这个文件」.
const READ_FILE_MISSING: i32 = 2;

/// Removes one remote path. Not being there is already the state the caller
/// asked for. The callers aim it at a directory they computed from that host's
/// own data directory (a version tree, the dedicated DSH_HOME) — and at the one
/// file an instance may own, the icon inside its HOME.
pub async fn remove_dir(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<(), HostError> {
    let out = transport
        .run(host, RemoteCall::new(remove_dir_script(path)))
        .await?;
    decode_ack(&out)
}

/// Whether that machine's npm knows one version of the launcher's package —
/// which decides between installing it from npm there and building it from
/// source there. `registry` empty means "use that machine's own npm
/// configuration".
pub async fn npm_has_version(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    package: &str,
    registry: Option<&str>,
) -> Result<bool, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        exists: bool,
    }
    let out = transport
        .run(
            host,
            RemoteCall::new(npm_has_version_script(package, registry)),
        )
        .await?;
    Ok(parse_marked::<Answer>(&out)?.exists)
}

// ---------------------------------------------------------------------------
// The environment probe
// ---------------------------------------------------------------------------

/// One candidate the probe passed over, and why. A remote host passes over a
/// Node below the recommended major and says so instead of using it silently
/// (ADR-0005 追加 2); the local host only warns about one and keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RemoteSkipped {
    pub path: String,
    #[serde(default)]
    pub version: String,
    /// `below-minimum` (its major is under the floor the launcher drives),
    /// `unparsable` (a version the launcher cannot read) or `unusable` (a
    /// hand-typed path that is not there or would not run). Codes, not
    /// sentences: the page renders them in the user's language.
    pub reason: String,
    /// The major the candidate reported, when the reason is about a version.
    #[serde(default)]
    pub major: Option<u64>,
}

/// One tool as a remote probe resolved it: where it is, what it says, which of
/// the three sources it came from, and everything passed over on the way.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RemoteToolStatus {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// `manual` | `system` | `fallback`; absent when nothing was found.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub skipped: Vec<RemoteSkipped>,
}

/// What one remote probe learned about a host's environment: the shell it was
/// read from, the version managers that machine has, and both tools resolved in
/// the launcher's fixed candidate order.
///
/// The machine's `PATH` is not carried back: the walk inside the probe uses it,
/// and everything the launcher does afterwards on that machine uses the absolute
/// paths the walk resolved (ADR-0005 追加 1).
#[derive(Clone, Debug, Deserialize)]
pub struct RemoteEnvironment {
    /// The shell the answer came from — that machine's bash.
    pub shell: String,
    #[serde(default)]
    pub managers: Vec<String>,
    pub node: RemoteToolStatus,
    pub pnpm: RemoteToolStatus,
}

/// Runs the environment probe on one host: a single interactive login shell that
/// reads `PATH`, walks the launcher's candidates in order and reports what each
/// one turned out to be.
///
/// The budget is larger than one command's: that shell starts an interactive
/// login shell (rc files included) and then runs `--version` for every candidate
/// it walks.
pub async fn environment(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    manual_node: Option<&str>,
    manual_pnpm: Option<&str>,
) -> Result<RemoteEnvironment, HostError> {
    let script = environment_script(manual_node, manual_pnpm);
    json::<RemoteEnvironment>(transport, host, script, ENVIRONMENT_TIMEOUT).await
}

/// How long the environment probe may take. Longer than one command's budget:
/// that shell starts an interactive login shell (rc files and all) and then runs
/// `--version` for every candidate it walks.
const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(60);

/// What one hand-typed path turned out to be on that machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualProbe {
    /// The file is there and answered `--version` with this.
    Version(String),
    /// There is no such file there.
    Missing,
    /// The file is there but would not run.
    Unusable,
}

/// Runs one hand-typed path's `--version` **on that machine**: the launcher
/// cannot execute a file on another computer, so this is the only way to accept
/// a path there at all (ADR-0005 追加 3).
pub async fn probe_manual_tool(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<ManualProbe, HostError> {
    let facts: ManualFacts =
        json(transport, host, manual_tool_script(path), DEFAULT_TIMEOUT).await?;
    Ok(match (facts.exists, facts.version) {
        (_, Some(version)) => ManualProbe::Version(version),
        (true, None) => ManualProbe::Unusable,
        (false, None) => ManualProbe::Missing,
    })
}

#[derive(Deserialize)]
struct ManualFacts {
    exists: bool,
    #[serde(default)]
    version: Option<String>,
}

/// The Linux install directories a remote probe searches after the machine's own
/// `PATH` (ADR-0005 追加 4): Linuxbrew, the nvm / fnm version directories, the
/// pnpm official script's directory, and the two user / system bin directories.
///
/// The probe walks exactly this list, so what the page reports is what any later
/// work on that machine will find; the wildcard entries are expanded there.
///
/// `$HOME` and the wildcards stay as written — only that machine's shell can
/// expand either, and expanding them here would read *this* machine's home.
pub fn fallback_bins() -> Vec<&'static str> {
    vec![
        "/home/linuxbrew/.linuxbrew/bin",
        "$HOME/.linuxbrew/bin",
        "$HOME/.nvm/versions/node/*/bin",
        "$HOME/.fnm/aliases/default/bin",
        "$HOME/.fnm/current/bin",
        "$HOME/.fnm/node-versions/*/installation/bin",
        "$HOME/.local/share/fnm/aliases/default/bin",
        "$HOME/.local/share/fnm/current/bin",
        "$HOME/.local/share/fnm/node-versions/*/installation/bin",
        "$HOME/.local/share/pnpm",
        "$HOME/.local/share/pnpm/bin",
        "$HOME/.local/bin",
        "/usr/local/bin",
    ]
}

/// One command the user runs **on this Mac** to run a script on a remote host.
///
/// Quoted for two shells because it goes through two: the user's own shell has
/// to hand ssh a single argument, and ssh joins its arguments with spaces and
/// lets the login shell at the other end split them again. The inner quotes are
/// what make the script reach `bash -ilc` as one word — the same shape
/// [`crate::ssh::script_command`] uses, so the command runs in the shell the
/// probe read.
///
/// The outer quoting is single-quote escaping rather than a plain pair of
/// quotes, so a script that quotes a path **itself** — `ls -la '<一个带空格的
/// 目录>'`, where the directory may also carry a single quote — survives the
/// user's own shell. A command therefore must not contain a double quote: the
/// inner pair would end early, and the ones below are written without one.
pub fn ssh_command(target: &str, recipe: &str) -> String {
    let shell = crate::ssh::REMOTE_SHELL;
    format!(
        "ssh {target} {}",
        quote(&format!("{shell} -ilc \"{recipe}\""))
    )
}

/// The Node install recipe for the machine the probe found these managers on:
/// extend the one the user already uses, so they end up with one Node in the
/// place they expect. With no manager at all it installs nvm and then loads it
/// into the same shell — a freshly installed `~/.bashrc` only takes effect in
/// the *next* shell, so the recipe has to source it itself.
pub fn node_install_recipe(managers: &[String]) -> &'static str {
    let has = |m: &str| managers.iter().any(|x| x == m);
    if has("nvm") {
        "nvm install --lts"
    } else if has("fnm") {
        "fnm install --lts"
    } else if has("brew") {
        "brew install node"
    } else {
        "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash \
         && export NVM_DIR=$HOME/.nvm && . $NVM_DIR/nvm.sh && nvm install --lts"
    }
}

/// The pnpm install recipe for a Linux machine: the official script rather than
/// `npm install -g pnpm`, because it needs no Node to run and it writes to
/// `~/.local/share/pnpm` — a directory the probe already searches, so a machine
/// that follows this instruction is found again without further setup.
pub fn pnpm_install_recipe() -> &'static str {
    "curl -fsSL https://get.pnpm.io/install.sh | sh -"
}

// ---------------------------------------------------------------------------
// Scripts
// ---------------------------------------------------------------------------

/// Escapes one value for the JSON the script builds. A login shell prints
/// things of its own, and a banner full of quotes must not be able to break the
/// answer apart: control characters go, backslashes and quotes are escaped.
const ESCAPER: &str = r#"dsh_esc() {
  printf '%s' "$1" | tr -d '\000-\037' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}"#;

/// The connection test: what this machine is, where its home and the launcher's
/// data directory are, and what `node` / `pnpm` resolve to.
///
/// It runs in an **interactive** login shell, which is the point: on a machine
/// where `~/.bashrc` puts Linuxbrew's Node 26 in front of apt's Node 18, only
/// that shell sees the Node the user's terminal sees — and execution later uses
/// the same shape, so the page and the work agree (ADR-0005 追加).
///
/// `%%DATA_DIR%%` is the host record's data directory, quoted, or empty.
const CONNECTION_TEST: &str = r#"set -u
%%ESCAPER%%

# A tool as this machine resolves it in this shell: an absolute path and the
# first line of its --version, or null when the machine has no such command.
dsh_tool() {
  dsh_path=$(command -v "$1" 2>/dev/null || true)
  if [ -z "$dsh_path" ]; then
    printf 'null'
    return 0
  fi
  dsh_version=$("$dsh_path" --version 2>/dev/null | head -n 1 || true)
  printf '{"path":"%s","version":"%s"}' "$(dsh_esc "$dsh_path")" "$(dsh_esc "$dsh_version")"
}

dsh_configured=%%DATA_DIR%%
case "$dsh_configured" in
  '') dsh_data="${HOME:-}/.local/share/dsh-launcher" ;;
  '~/'*) dsh_data="${HOME:-}/${dsh_configured:2}" ;;
  *) dsh_data="$dsh_configured" ;;
esac
if [ -d "$dsh_data" ]; then dsh_exists=true; else dsh_exists=false; fi

dsh_json=$(printf '{"os":"%s","release":"%s","arch":"%s","home":"%s","data_dir":"%s","data_dir_exists":%s,"node":%s,"pnpm":%s}' \
  "$(dsh_esc "$(uname -s)")" \
  "$(dsh_esc "$(uname -r)")" \
  "$(dsh_esc "$(uname -m)")" \
  "$(dsh_esc "${HOME:-}")" \
  "$(dsh_esc "$dsh_data")" \
  "$dsh_exists" \
  "$(dsh_tool node)" \
  "$(dsh_tool pnpm)")
printf '<<<DSH-JSON>>>%s<<<DSH-END>>>' "$dsh_json"
"#;

/// The versions installed under one directory, with whether each one's bin is
/// ready.
///
/// The layout is [`crate::launch::version_bin`]'s, applied on that machine: a
/// repo checkout carries `apps/cli/lib/bin.js`, an npm tree carries
/// `node_modules/@deepseek-ai/dsh/lib/bin.js`, and a version whose install
/// stopped early has neither. A versions directory that is not there yet is an
/// empty list — a machine with nothing installed is a machine, not an error.
const INSTALLED_VERSIONS: &str = r#"set -u
%%ESCAPER%%
dsh_dir=%%PATH%%
dsh_rows=
dsh_sep=
for dsh_entry in "$dsh_dir"/*; do
  [ -d "$dsh_entry" ] || continue
  if [ -f "$dsh_entry/apps/cli/package.json" ]; then
    dsh_bin="$dsh_entry/apps/cli/lib/bin.js"
  else
    dsh_bin="$dsh_entry/node_modules/@deepseek-ai/dsh/lib/bin.js"
  fi
  if [ -s "$dsh_bin" ]; then dsh_ready=true; else dsh_ready=false; fi
  dsh_rows="$dsh_rows$dsh_sep{\"name\":\"$(dsh_esc "${dsh_entry##*/}")\",\"dir\":\"$(dsh_esc "$dsh_entry")\",\"bin_ready\":$dsh_ready}"
  dsh_sep=,
done
printf '<<<DSH-JSON>>>{"versions":[%s]}<<<DSH-END>>>' "$dsh_rows"
"#;

/// `@deepseek-ai/dsh` as **that machine's** npm sees it, against the registry
/// its host record names — a machine whose registry is its own can install
/// versions this Mac's npm has never heard of, and listing them from here would
/// offer the user a list that cannot be installed there.
///
/// The two listings travel as JSON *text*, the shape the local `npm view` hands
/// over, so one conversion serves both sources.
const AVAILABLE_VERSIONS: &str = r#"set -u
%%ESCAPER%%
dsh_npm=$(command -v npm 2>/dev/null || true)
if [ -z "$dsh_npm" ]; then
  printf '这台机器上没有 npm\n' >&2
  exit 2
fi
dsh_versions=$("$dsh_npm" view @deepseek-ai/dsh versions --json --registry %%REGISTRY%% 2>/dev/null || true)
if [ -z "$dsh_versions" ]; then
  printf 'npm view 没有从 %%REGISTRY%% 拿到版本列表\n' >&2
  exit 3
fi
dsh_time=$("$dsh_npm" view @deepseek-ai/dsh time --json --registry %%REGISTRY%% 2>/dev/null || true)
dsh_json=$(printf '{"versions":"%s","time":"%s"}' "$(dsh_esc "$dsh_versions")" "$(dsh_esc "$dsh_time")")
printf '<<<DSH-JSON>>>%s<<<DSH-END>>>' "$dsh_json"
"#;

/// The installed-versions script for one host's `versions` directory (that
/// host's data directory plus `/versions`, which the caller resolves).
pub fn installed_versions_script(versions_dir: &str) -> String {
    fill(
        INSTALLED_VERSIONS,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATH%%", quote(versions_dir)),
        ],
    )
}

/// The versions-query script for one registry.
pub fn available_versions_script(registry: &str) -> String {
    fill(
        AVAILABLE_VERSIONS,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%REGISTRY%%", quote(registry)),
        ],
    )
}

/// The connection test's script. `data_dir` is the host's configured data
/// directory (`None` = let that machine pick its default).
pub fn connection_test_script(data_dir: Option<&str>) -> String {
    fill(
        CONNECTION_TEST,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%DATA_DIR%%", data_dir.map(quote).unwrap_or_default()),
        ],
    )
}

/// The environment probe: what this machine's **interactive login shell** gives,
/// and what the launcher's candidates resolve to when walked in order.
///
/// Interactive *and* login, not just login: on Linux `~/.bashrc` is where
/// brew, nvm, fnm and miniconda put themselves on `PATH`, and only an
/// interactive shell reads it — a `bash -lc` would miss the very Node the user's
/// terminal runs (ADR-0005 追加 1). Execution later uses the same shell shape,
/// so what the page reports and what runs are the same thing.
///
/// A Node below `%%NODE_MIN%%` is passed over and the walk continues; only when
/// nothing qualifies is the newest one below the floor used, and it stays
/// reported as passed over (ADR-0005 追加 2). pnpm below its floor drives no
/// install, so there the walk simply finds nothing — the same call the local
/// resolver makes.
const ENVIRONMENT: &str = r#"set -u
%%ESCAPER%%

dsh_node_min=%%NODE_MIN%%
dsh_pnpm_min=%%PNPM_MIN%%
dsh_manual_node=%%MANUAL_NODE%%
dsh_manual_pnpm=%%MANUAL_PNPM%%
dsh_fallback=%%FALLBACK%%

# One candidate's version, or nothing when it is missing or will not run.
dsh_version_of() {
  [ -f "$1" ] || return 1
  dsh_said=$("$1" --version 2>/dev/null | head -n 1)
  [ -n "$dsh_said" ] || return 1
  printf '%s' "$dsh_said"
}

# Every candidate, one per line, each preceded by the source it came from: what
# the user picked, then the directories this interactive login shell has on
# PATH, then the Linux install directories. A wildcard entry (an nvm or fnm
# version directory) is expanded here and sorted newest first, which is the
# order the local scan prefers too.
dsh_candidates() {
  dsh_prog=$1
  dsh_manual=$2
  if [ -n "$dsh_manual" ]; then printf 'manual\t%s\n' "$dsh_manual"; fi
  dsh_ifs=$IFS
  IFS=:
  for dsh_dir in $PATH; do
    [ -n "$dsh_dir" ] || continue
    printf 'system\t%s\n' "$dsh_dir/$dsh_prog"
  done
  for dsh_dir in $dsh_fallback; do
    IFS=$dsh_ifs
    [ -n "$dsh_dir" ] || continue
    case "$dsh_dir" in
      *\**)
        for dsh_hit in $(ls -d "$dsh_dir" 2>/dev/null | sort -Vr); do
          printf 'fallback\t%s\n' "$dsh_hit/$dsh_prog"
        done
        ;;
      *) printf 'fallback\t%s\n' "$dsh_dir/$dsh_prog" ;;
    esac
    IFS=:
  done
  IFS=$dsh_ifs
}

# One tool's answer: the candidate that was taken, and everything passed over.
# `dsh_last_resort` says whether an older candidate beats having none at all
# (Node is needed to boot DSH; pnpm below its floor drives nothing), and
# `dsh_need_major` says whether a version the launcher cannot read disqualifies
# a candidate (pnpm, which is driven with flags pinned to a major).
dsh_pick() {
  dsh_prog=$1
  dsh_min=$2
  dsh_manual=$3
  dsh_last_resort=$4
  dsh_need_major=$5
  dsh_taken_source=
  dsh_taken_path=
  dsh_taken_version=
  dsh_below_source=
  dsh_below_path=
  dsh_below_version=
  dsh_skipped=
  dsh_sep=

  while read -r dsh_source dsh_path; do
    dsh_version=$(dsh_version_of "$dsh_path") || {
      if [ "$dsh_source" = manual ]; then
        dsh_skipped="$dsh_skipped$dsh_sep{\"path\":\"$(dsh_esc "$dsh_path")\",\"version\":\"\",\"reason\":\"unusable\"}"
        dsh_sep=,
      fi
      continue
    }
    dsh_major=$(printf '%s' "$dsh_version" | sed -e 's/^v//' | cut -d. -f1 | tr -cd '0-9')
    if [ -z "$dsh_major" ]; then
      if [ "$dsh_need_major" = yes ]; then
        dsh_skipped="$dsh_skipped$dsh_sep{\"path\":\"$(dsh_esc "$dsh_path")\",\"version\":\"$(dsh_esc "$dsh_version")\",\"reason\":\"unparsable\"}"
        dsh_sep=,
        continue
      fi
    elif [ "$dsh_major" -lt "$dsh_min" ]; then
      dsh_skipped="$dsh_skipped$dsh_sep{\"path\":\"$(dsh_esc "$dsh_path")\",\"version\":\"$(dsh_esc "$dsh_version")\",\"reason\":\"below-minimum\",\"major\":$dsh_major}"
      dsh_sep=,
      dsh_below_source=$dsh_source
      dsh_below_path=$dsh_path
      dsh_below_version=$dsh_version
      continue
    fi
    dsh_taken_source=$dsh_source
    dsh_taken_path=$dsh_path
    dsh_taken_version=$dsh_version
    break
  done < <(dsh_candidates "$dsh_prog" "$dsh_manual")

  if [ -z "$dsh_taken_path" ] && [ "$dsh_last_resort" = yes ] && [ -n "$dsh_below_path" ]; then
    dsh_taken_source=$dsh_below_source
    dsh_taken_path=$dsh_below_path
    dsh_taken_version=$dsh_below_version
  fi

  if [ -z "$dsh_taken_path" ]; then
    printf '{"path":null,"version":null,"source":null,"skipped":[%s]}' "$dsh_skipped"
  else
    printf '{"path":"%s","version":"%s","source":"%s","skipped":[%s]}' \
      "$(dsh_esc "$dsh_taken_path")" "$(dsh_esc "$dsh_taken_version")" "$dsh_taken_source" "$dsh_skipped"
  fi
}

dsh_managers=
dsh_msep=
if [ -d "${HOME:-}/.nvm" ]; then dsh_managers="$dsh_managers$dsh_msep\"nvm\""; dsh_msep=,; fi
if [ -d "${HOME:-}/.fnm" ] || [ -d "${HOME:-}/.local/share/fnm" ]; then dsh_managers="$dsh_managers$dsh_msep\"fnm\""; dsh_msep=,; fi
if [ -x /home/linuxbrew/.linuxbrew/bin/brew ] || [ -x /usr/local/bin/brew ]; then dsh_managers="$dsh_managers$dsh_msep\"brew\""; dsh_msep=,; fi

dsh_json=$(printf '{"shell":"%s","managers":[%s],"node":%s,"pnpm":%s}' \
  "$(dsh_esc "${BASH:-/bin/bash}")" \
  "$dsh_managers" \
  "$(dsh_pick node "$dsh_node_min" "$dsh_manual_node" yes no)" \
  "$(dsh_pick pnpm "$dsh_pnpm_min" "$dsh_manual_pnpm" no yes)")
printf '<<<DSH-JSON>>>%s<<<DSH-END>>>' "$dsh_json"
"#;

/// The environment probe's script. `manual_node` / `manual_pnpm` are the user's
/// hand-typed paths on that machine, or `None`.
pub fn environment_script(manual_node: Option<&str>, manual_pnpm: Option<&str>) -> String {
    environment_script_with(manual_node, manual_pnpm, &fallback_bins())
}

/// [`environment_script`] with the install-directory list injected, the way
/// `runtime::candidates_with` injects the local one: the walk over that list is
/// what has to be driven against a fixture, because the test machine's own home
/// directory is not the remote machine's.
pub(crate) fn environment_script_with(
    manual_node: Option<&str>,
    manual_pnpm: Option<&str>,
    fallback: &[&str],
) -> String {
    fill(
        ENVIRONMENT,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            (
                "%%NODE_MIN%%",
                crate::runtime::RECOMMENDED_NODE_MAJOR.to_string(),
            ),
            (
                "%%PNPM_MIN%%",
                crate::toolchain::MIN_PNPM_MAJOR.to_string(),
            ),
            (
                "%%MANUAL_NODE%%",
                manual_node.map(quote).unwrap_or_default(),
            ),
            (
                "%%MANUAL_PNPM%%",
                manual_pnpm.map(quote).unwrap_or_default(),
            ),
            ("%%FALLBACK%%", fallback.join(":")),
        ],
    )
}

/// The probe of one hand-typed path: is it there, and does it answer
/// `--version`. The order is the answer — the file has to exist before the
/// launcher judges whether it runs.
const MANUAL_TOOL: &str = r#"set -u
%%ESCAPER%%
dsh_path=%%PATH%%
dsh_exists=false
dsh_version=
if [ -f "$dsh_path" ]; then
  dsh_exists=true
  dsh_version=$("$dsh_path" --version 2>/dev/null | head -n 1)
fi
dsh_said=null
if [ -n "$dsh_version" ]; then dsh_said="\"$(dsh_esc "$dsh_version")\""; fi
printf '<<<DSH-JSON>>>{"exists":%s,"version":%s}<<<DSH-END>>>' "$dsh_exists" "$dsh_said"
"#;

pub fn manual_tool_script(path: &str) -> String {
    fill(
        MANUAL_TOOL,
        &[("%%ESCAPER%%", ESCAPER.to_string()), ("%%PATH%%", quote(path))],
    )
}

// ---------------------------------------------------------------------------
// DSH_HOME 与 Profile
// ---------------------------------------------------------------------------

/// Makes sure one DSH_HOME exists on that machine and answers where it is.
///
/// The path is the user's, typed for a machine they are not sitting at, so
/// **that machine** is what resolves it: `~` expands against its own `$HOME`
/// (the launcher's `normalize_dir_path` would expand it against this Mac's),
/// `..` rows are resolved as text the way the local rule does — no symlink
/// resolution, so the path the record keeps is the path the user wrote in its
/// settled spelling — and the directory is created if it is not there yet,
/// exactly as [`crate::commands::create_home_record_in`] does for 本机.
///
/// A path that cannot be used there fails here, with what the machine said,
/// instead of becoming a record pointing at a directory that does not exist
/// where the instance will run.
const HOME_PATH: &str = r#"set -u
%%ESCAPER%%
dsh_raw=%%PATH%%
case "$dsh_raw" in
  '~') dsh_requested="${HOME:-}" ;;
  '~/'*) dsh_requested="${HOME:-}/${dsh_raw#\~/}" ;;
  '~'*) printf '不支持 ~用户名 形式的路径：%s\n' "$dsh_raw" >&2; exit 2 ;;
  *) dsh_requested="$dsh_raw" ;;
esac
case "$dsh_requested" in
  /*) ;;
  *) printf '请填写绝对路径（以 / 开头）：%s\n' "$dsh_raw" >&2; exit 3 ;;
esac

# Settle the path by text: one directory typed two ways is one DSH_HOME.
dsh_rest=${dsh_requested#/}
dsh_path=
while [ -n "$dsh_rest" ]; do
  dsh_part=${dsh_rest%%/*}
  if [ "$dsh_rest" = "$dsh_part" ]; then dsh_rest=; else dsh_rest=${dsh_rest#*/}; fi
  case "$dsh_part" in
    ''|.) ;;
    ..) dsh_path=${dsh_path%/*} ;;
    *) dsh_path="$dsh_path/$dsh_part" ;;
  esac
done
[ -n "$dsh_path" ] || dsh_path=/

if [ -e "$dsh_path" ] && [ ! -d "$dsh_path" ]; then
  printf '这个路径上已经有一个文件：%s\n' "$dsh_path" >&2
  exit 4
fi
if ! mkdir -p "$dsh_path"; then
  printf '不能创建这个目录：%s\n' "$dsh_path" >&2
  exit 5
fi
if [ ! -w "$dsh_path" ]; then
  printf '这个目录不可写：%s\n' "$dsh_path" >&2
  exit 6
fi
printf '<<<DSH-JSON>>>{"path":"%s"}<<<DSH-END>>>' "$(dsh_esc "$dsh_path")"
"#;

/// That machine's home directory, and whether it already has a `~/.dsh` —
/// [`crate::config::ensure_user_dsh_home`]'s question asked of the machine that
/// can answer it (that sweep expands `~` against *this* Mac's home, so it can
/// only ever be about 本机).
const USER_HOME: &str = r#"set -u
%%ESCAPER%%
dsh_user_home=${HOME:-}
dsh_dsh_home=
dsh_exists=false
if [ -n "$dsh_user_home" ] && [ -d "$dsh_user_home/.dsh" ]; then
  dsh_dsh_home="$dsh_user_home/.dsh"
  dsh_exists=true
fi
printf '<<<DSH-JSON>>>{"home":"%s","dsh_home":"%s","exists":%s}<<<DSH-END>>>' \
  "$(dsh_esc "$dsh_user_home")" "$(dsh_esc "$dsh_dsh_home")" "$dsh_exists"
"#;

/// The profile directories under one DSH_HOME on that machine. Directories
/// only, and the two reserved names left out, so the list is the same list
/// [`crate::profile::profiles_dir`] yields here. A HOME with no `profiles/`
/// yet is an empty list, not an error.
const PROFILES: &str = r#"set -u
%%ESCAPER%%
dsh_dir=%%DIR%%
dsh_names=
dsh_sep=
for dsh_entry in "$dsh_dir"/*/ "$dsh_dir"/.[!.]*/; do
  [ -d "$dsh_entry" ] || continue
  dsh_name=${dsh_entry%/}
  dsh_name=${dsh_name##*/}
  case "$dsh_name" in
    node_modules|__temp__) continue ;;
  esac
  dsh_names="$dsh_names$dsh_sep\"$(dsh_esc "$dsh_name")\""
  dsh_sep=,
done
printf '<<<DSH-JSON>>>{"profiles":[%s]}<<<DSH-END>>>' "$dsh_names"
"#;

/// Copies one profile directory on that machine. The checks mirror the local
/// flow's, so both machines answer the same situation the same way.
const COPY_PROFILE: &str = r#"set -u
dsh_from=%%FROM%%
dsh_to=%%TO%%
if [ ! -d "$dsh_from" ]; then
  printf '远端没有这个 Profile 目录：%s\n' "$dsh_from" >&2
  exit 2
fi
if [ -e "$dsh_to" ]; then
  printf '远端已经有这个目录：%s\n' "$dsh_to" >&2
  exit 3
fi
if ! mkdir -p "$(dirname "$dsh_to")"; then
  printf '不能创建目录：%s\n' "$(dirname "$dsh_to")" >&2
  exit 4
fi
if ! cp -R "$dsh_from" "$dsh_to"; then
  printf '复制失败：%s -> %s\n' "$dsh_from" "$dsh_to" >&2
  exit 5
fi
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

/// Renames one profile directory on that machine.
const RENAME_PROFILE: &str = r#"set -u
dsh_from=%%FROM%%
dsh_to=%%TO%%
if [ ! -d "$dsh_from" ]; then
  printf '远端没有这个 Profile 目录：%s\n' "$dsh_from" >&2
  exit 2
fi
if [ -e "$dsh_to" ]; then
  printf '远端已经有这个目录：%s\n' "$dsh_to" >&2
  exit 3
fi
if ! mv "$dsh_from" "$dsh_to"; then
  printf '改名失败：%s -> %s\n' "$dsh_from" "$dsh_to" >&2
  exit 4
fi
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

/// Removes one profile directory on that machine.
const DELETE_PROFILE: &str = r#"set -u
dsh_target=%%PATH%%
if [ ! -d "$dsh_target" ]; then
  printf '远端没有这个 Profile 目录：%s\n' "$dsh_target" >&2
  exit 2
fi
if ! rm -rf "$dsh_target"; then
  printf '删除失败：%s\n' "$dsh_target" >&2
  exit 3
fi
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

/// One profile's own files on the machine that owns it: the `package.json` the
/// profile declares its layers and dependencies in, and the version of every
/// package installed under its `node_modules`.
///
/// One call for the whole profile, not one per package: this is the projection
/// the plugins page falls back to whenever no instance answers, and a connection
/// per file would make the fallback slower than what it stands in for (the same
/// rule the dependency self-check follows). The `package.json` travels as base64
/// because it is a document and not a line of text; each version is picked out
/// of its own manifest here, so a caller gets the one field it needs without a
/// round trip per package.
const PROFILE_PACKAGES: &str = r#"set -u
%%ESCAPER%%
dsh_dir=%%PATH%%
dsh_manifest=null
if [ -f "$dsh_dir/package.json" ]; then
  dsh_manifest="\"$(base64 < "$dsh_dir/package.json" | tr -d '\n')\""
fi

# One installed package: the name the profile spells it by (the directory's own
# name, scope included) and the version its manifest states. A manifest that
# states none leaves the version empty, which reads as 「不知道版本」.
dsh_row() {
  dsh_pkg="$2/package.json"
  [ -f "$dsh_pkg" ] || return 0
  dsh_version=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$dsh_pkg" 2>/dev/null | head -n 1 || true)
  if [ -z "$dsh_version" ]; then
    dsh_version=$(tr -d '\n' < "$dsh_pkg" | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 || true)
  fi
  dsh_version=$(printf '%s' "$dsh_version" | cut -d'"' -f4)
  dsh_rows="$dsh_rows$dsh_sep{\"name\":\"$(dsh_esc "$1")\",\"version\":\"$(dsh_esc "$dsh_version")\"}"
  dsh_sep=,
}

dsh_rows=
dsh_sep=
dsh_nm="$dsh_dir/node_modules"
if [ -d "$dsh_nm" ]; then
  for dsh_entry in "$dsh_nm"/*; do
    [ -d "$dsh_entry" ] || continue
    dsh_leaf=${dsh_entry##*/}
    case "$dsh_leaf" in
      .*) ;;
      @*)
        for dsh_inner in "$dsh_entry"/*; do
          [ -d "$dsh_inner" ] || continue
          dsh_row "$dsh_leaf/${dsh_inner##*/}" "$dsh_inner"
        done
        ;;
      *) dsh_row "$dsh_leaf" "$dsh_entry" ;;
    esac
  done
fi
printf '<<<DSH-JSON>>>{"manifest":%s,"installed":[%s]}<<<DSH-END>>>' "$dsh_manifest" "$dsh_rows"
"#;

pub fn home_path_script(path: &str) -> String {
    fill(
        HOME_PATH,
        &[("%%ESCAPER%%", ESCAPER.to_string()), ("%%PATH%%", quote(path))],
    )
}

pub fn user_home_script() -> String {
    fill(USER_HOME, &[("%%ESCAPER%%", ESCAPER.to_string())])
}

pub fn profiles_script(profiles_dir: &str) -> String {
    fill(
        PROFILES,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%DIR%%", quote(profiles_dir)),
        ],
    )
}

pub fn copy_profile_script(from: &str, to: &str) -> String {
    fill(
        COPY_PROFILE,
        &[("%%FROM%%", quote(from)), ("%%TO%%", quote(to))],
    )
}

pub fn rename_profile_script(from: &str, to: &str) -> String {
    fill(
        RENAME_PROFILE,
        &[("%%FROM%%", quote(from)), ("%%TO%%", quote(to))],
    )
}

pub fn delete_profile_script(path: &str) -> String {
    fill(DELETE_PROFILE, &[("%%PATH%%", quote(path))])
}

/// The script that reads one profile's own files on that machine.
pub fn profile_packages_script(profile_dir: &str) -> String {
    fill(
        PROFILE_PACKAGES,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATH%%", quote(profile_dir)),
        ],
    )
}

/// Creates (or finds) that machine's DSH_HOME for `path` and answers its
/// absolute path **as that machine resolves it**.
pub async fn ensure_home_path(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<String, HostError> {
    let answer: HomePath = json(transport, host, home_path_script(path), DEFAULT_TIMEOUT).await?;
    Ok(answer.path)
}

/// That machine's home directory, and whether it already has a `~/.dsh`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RemoteUserHome {
    pub home: String,
    pub dsh_home: String,
    pub exists: bool,
}

pub async fn user_home(
    transport: &dyn HostTransport,
    host: &RemoteHost,
) -> Result<RemoteUserHome, HostError> {
    json(transport, host, user_home_script(), DEFAULT_TIMEOUT).await
}

/// The profile directories under one DSH_HOME on that machine.
pub async fn list_profiles(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    profiles_dir: &str,
) -> Result<Vec<String>, HostError> {
    let out = transport
        .run(host, RemoteCall::new(profiles_script(profiles_dir)))
        .await?;
    decode_profiles(&out)
}

/// Copies one profile directory, on that machine.
pub async fn copy_profile(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    from: &str,
    to: &str,
) -> Result<(), HostError> {
    let out = transport
        .run(host, RemoteCall::new(copy_profile_script(from, to)))
        .await?;
    decode_ack(&out)
}

/// Renames one profile directory, on that machine.
pub async fn rename_profile(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    from: &str,
    to: &str,
) -> Result<(), HostError> {
    let out = transport
        .run(host, RemoteCall::new(rename_profile_script(from, to)))
        .await?;
    decode_ack(&out)
}

/// Removes one profile directory, on that machine.
pub async fn delete_profile(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<(), HostError> {
    let out = transport
        .run(host, RemoteCall::new(delete_profile_script(path)))
        .await?;
    decode_ack(&out)
}

/// One profile's own files, as that machine answered: the `package.json` it
/// declares its layers in (its bytes, or `None` when the profile has none), and
/// the version of every package installed under its `node_modules`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteProfilePackages {
    pub manifest: Option<Vec<u8>>,
    pub installed: std::collections::BTreeMap<String, String>,
}

/// Reads one profile's own files on the machine that owns it, in one call.
///
/// Nothing here is a claim about the instance: this is the file projection the
/// plugins page uses when no instance answers (ADR-0001 — the running DSH owns
/// plugin state), and it stays thinner than a live list on purpose.
pub async fn profile_packages(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    profile_dir: &str,
) -> Result<RemoteProfilePackages, HostError> {
    let out = transport
        .run(host, RemoteCall::new(profile_packages_script(profile_dir)))
        .await?;
    decode_profile_packages(&out)
}

/// Reads one file, as base64 — the ssh channel is text, and version manifests,
/// icons and patch documents must arrive byte for byte.
const READ_FILE: &str = r#"set -u
dsh_path=%%PATH%%
if [ ! -f "$dsh_path" ]; then
  printf '远端没有这个文件：%s\n' "$dsh_path" >&2
  exit 2
fi
dsh_data=$(base64 < "$dsh_path" | tr -d '\n')
printf '<<<DSH-JSON>>>{"data":"%s"}<<<DSH-END>>>' "$dsh_data"
"#;

/// Writes one file, creating its directory. The bytes travel base64-encoded in
/// the script itself: the payloads the launcher writes (an icon, a small JSON
/// document) are far below the argv limit.
const WRITE_FILE: &str = r#"set -u
dsh_path=%%PATH%%
mkdir -p "$(dirname "$dsh_path")" || exit 2
printf %s %%BASE64%% | base64 -d > "$dsh_path" || exit 3
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

/// Lists one directory's entry names (hidden files included).
const LIST_DIR: &str = r#"set -u
%%ESCAPER%%
dsh_path=%%PATH%%
if [ ! -d "$dsh_path" ]; then
  printf '远端没有这个目录：%s\n' "$dsh_path" >&2
  exit 2
fi
dsh_names=
dsh_sep=
for dsh_entry in "$dsh_path"/* "$dsh_path"/.[!.]*; do
  [ -e "$dsh_entry" ] || continue
  dsh_names="$dsh_names$dsh_sep\"$(dsh_esc "${dsh_entry##*/}")\""
  dsh_sep=,
done
printf '<<<DSH-JSON>>>{"entries":[%s]}<<<DSH-END>>>' "$dsh_names"
"#;

/// Stops one process. SIGTERM by pid, with the pid cross-checked against the
/// running record by the caller before it gets here (ADR-0007): a pid may have
/// been reused, and signalling the wrong process is the one unrecoverable
/// mistake available on a remote machine.
const STOP_PID: &str = r#"set -u
kill -TERM %%PID%% 2>/dev/null || { printf '不能给进程 %s 发信号\n' %%PID%% >&2; exit 3; }
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

/// Stops a whole process tree: the process group one detached spawn leads
/// (its [`SPAWN`] ran under `setsid`, so its pid is the group's id), falling
/// back to the process itself when it leads no group.
///
/// Killing only the leader is not enough for the case this exists for: a
/// cancelled `pnpm install` leaves its helpers behind, and a helper holding the
/// store's lock makes every later install on that machine wait.
///
/// Already gone counts as stopped — a second cancel, or an install that exited
/// between the click and this call, is not a failure.
const STOP_GROUP: &str = r#"set -u
dsh_pid=%%PID%%
if kill -TERM -- "-$dsh_pid" 2>/dev/null || kill -TERM "$dsh_pid" 2>/dev/null; then
  printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
elif kill -0 -- "-$dsh_pid" 2>/dev/null || kill -0 "$dsh_pid" 2>/dev/null; then
  printf '进程 %s 还在，但发不了终止信号（权限不足？）\n' "$dsh_pid" >&2
  exit 3
else
  printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
fi
"#;

/// Whether the process tree one detached spawn started is still there. The same
/// predicate [`STOP_GROUP`] signals with, so 「停了」 and 「真的没了」 cannot
/// disagree.
const ALIVE: &str = r#"set -u
dsh_pid=%%PID%%
if kill -0 -- "-$dsh_pid" 2>/dev/null || kill -0 "$dsh_pid" 2>/dev/null; then
  printf '<<<DSH-JSON>>>{"alive":true}<<<DSH-END>>>'
else
  printf '<<<DSH-JSON>>>{"alive":false}<<<DSH-END>>>'
fi
"#;

/// Who holds one TCP port on that machine, as `ss` names it: the pid of the
/// listening process, and the short command name `ps` reports for that pid.
///
/// `ss` is where a port's occupant is visible — and unlike a probe done from
/// here, it answers about **that** machine's listeners (ADR-0006). The number
/// alone is not enough to move the occupant out of the way, so the name is read
/// too; a listener this user does not own comes back without a pid, which is
/// still the answer "occupied, by somebody else".
const PORT_OWNER: &str = r#"set -u
dsh_port=%%PORT%%
dsh_row=$(ss -ltnp 2>/dev/null | grep -E ":${dsh_port}[[:space:]]" | head -n 1)
if [ -z "$dsh_row" ]; then
  printf '<<<DSH-JSON>>>{"occupied":false,"pid":null,"command":""}<<<DSH-END>>>'
  exit 0
fi
dsh_pid=$(printf '%s' "$dsh_row" | grep -o 'pid=[0-9]*' | head -n 1 | cut -d= -f2)
if [ -z "$dsh_pid" ]; then
  printf '<<<DSH-JSON>>>{"occupied":true,"pid":null,"command":""}<<<DSH-END>>>'
  exit 0
fi
dsh_name=$(ps -p "$dsh_pid" -o comm= 2>/dev/null | head -n 1 | tr -d '\r\n"\\')
printf '<<<DSH-JSON>>>{"occupied":true,"pid":%s,"command":"%s"}<<<DSH-END>>>' "$dsh_pid" "$dsh_name"
"#;

/// Waits **on that machine** until one process tree is gone, or the budget runs
/// out. The same predicate [`ALIVE`] reports with, so 「信号发出去了」 and 「真的
/// 没了」 cannot disagree.
///
/// The wait belongs over there for the same reason [`TAIL`]'s does: confirming a
/// stop by polling from here would open one ssh per second, when the only fact
/// needed is when that machine stops seeing the process.
const WAIT_GONE: &str = r#"set -u
dsh_pid=%%PID%%
dsh_wait=%%WAIT%%

dsh_alive() {
  kill -0 -- "-$dsh_pid" 2>/dev/null || kill -0 "$dsh_pid" 2>/dev/null
}

dsh_i=0
while [ "$dsh_i" -lt "$dsh_wait" ] && dsh_alive; do
  sleep 1
  dsh_i=$((dsh_i + 1))
done
if dsh_alive; then
  printf '<<<DSH-JSON>>>{"gone":false}<<<DSH-END>>>'
else
  printf '<<<DSH-JSON>>>{"gone":true}<<<DSH-END>>>'
fi
"#;

/// Exchanges one instance URL's launch token for its session cookie **on that
/// machine**, and reports what came back: the HTTP status and the raw
/// `Set-Cookie` pair.
///
/// The request has to be made there. A remote instance binds that machine's own
/// `127.0.0.1`, so asking the same URL from 本机 would be asking this Mac about a
/// different service — the confusion ADR-0006 exists to prevent, and the reason
/// the token cannot be trusted by trying it from here. Deciding whether the
/// answer means 「认证成功」 is [`crate::native`]'s rule (the cookie name is bound
/// to the authority); this script only carries the response.
///
/// No `curl` on that machine reads as status 0 with no cookie: 「问不出来」, which
/// the caller treats as 「没有认证成功」 — never as success.
const MINT_REMOTE: &str = r#"set -u
dsh_url=%%URL%%
dsh_curl=$(command -v curl 2>/dev/null || true)
if [ -z "$dsh_curl" ]; then
  printf '<<<DSH-JSON>>>{"status":0,"cookie":null}<<<DSH-END>>>'
  exit 0
fi
dsh_head=$("$dsh_curl" -s -S -o /dev/null -D - --max-time 10 "$dsh_url" 2>/dev/null || true)
dsh_status=$(printf '%s\n' "$dsh_head" | awk 'toupper($1) ~ /^HTTP\// {code=$2} END {print code+0}')
dsh_cookie=$(printf '%s\n' "$dsh_head" | awk 'tolower($1) == "set-cookie:" {print $2; exit}' | tr -d '\r')
if [ -z "$dsh_cookie" ]; then
  printf '<<<DSH-JSON>>>{"status":%s,"cookie":null}<<<DSH-END>>>' "$dsh_status"
else
  printf '<<<DSH-JSON>>>{"status":%s,"cookie":"%s"}<<<DSH-END>>>' "$dsh_status" "$dsh_cookie"
fi
"#;

/// The smallest thing the launcher can ask a machine to do: exit 0. It exists so
/// the launcher's multiplexed connection is up before something has to ride it —
/// `ssh -O forward` is a request to a master, it does not start one (ADR-0008).
const HELLO: &str = "true";

pub fn hello_script() -> String {
    HELLO.to_string()
}

/// How many bytes a log already holds. A file that is not there yet is zero:
/// every caller here reads the size of a log it is about to append to.
const LOG_SIZE: &str = r#"set -u
dsh_bytes=$(wc -c < %%LOG%% 2>/dev/null | tr -d ' ')
if [ -z "$dsh_bytes" ]; then dsh_bytes=0; fi
printf '<<<DSH-JSON>>>{"offset":%s}<<<DSH-END>>>' "$dsh_bytes"
"#;

/// Whether one file exists and is not empty (`-s`), the rule a version's
/// `bin.js` is checked with on 本机.
const FILE_READY: &str = r#"set -u
if [ -s %%PATH%% ]; then dsh_ready=true; else dsh_ready=false; fi
printf '<<<DSH-JSON>>>{"ready":%s}<<<DSH-END>>>' "$dsh_ready"
"#;

/// When one pid started, in clock ticks since boot: field 22 of
/// `/proc/<pid>/stat`, which puts a pid number back together with the process it
/// names.
///
/// The fields are counted from after the parenthesised command name: that name
/// is free text and may contain spaces, so `$22` of the whole line would drift.
/// Anything that is not a number (the process is gone, `/proc` is not mounted)
/// reads as null — the caller's 「查不了」.
const START_TIME: &str = r#"set -u
dsh_pid=%%PID%%
dsh_stat=$(cat "/proc/$dsh_pid/stat" 2>/dev/null || true)
dsh_rest=${dsh_stat##*') '}
dsh_started=$(printf '%s' "$dsh_rest" | awk '{print $20}' 2>/dev/null || true)
case "$dsh_started" in
  ''|*[!0-9]*) printf '<<<DSH-JSON>>>{"started_at":null}<<<DSH-END>>>' ;;
  *) printf '<<<DSH-JSON>>>{"started_at":%s}<<<DSH-END>>>' "$dsh_started" ;;
esac
"#;

/// The tail of one remote log from a byte offset, waiting on that machine until
/// there is something to report.
///
/// A remote install has no pipe to read: it runs detached so it outlives the
/// ssh session, and its only output is a log file. Reading that file once per
/// poll would open an ssh per second for the ten minutes an install takes, so
/// the wait happens **there** — this script returns as soon as the log has grown
/// past the offset, the process is gone, or the budget runs out. Live output
/// therefore arrives about a second after it is written, and a quiet install
/// costs one call per `%%WAIT%%` seconds.
///
/// The bytes travel base64-encoded: the channel is text, and pnpm's output
/// carries whatever its own terminals printed.
const TAIL: &str = r#"set -u
dsh_log=%%LOG%%
dsh_offset=%%OFFSET%%
dsh_pid=%%PID%%
dsh_wait=%%WAIT%%

dsh_alive() {
  if [ "$dsh_pid" -le 0 ]; then return 0; fi
  kill -0 -- "-$dsh_pid" 2>/dev/null || kill -0 "$dsh_pid" 2>/dev/null
}

dsh_size() {
  dsh_bytes=$(wc -c < "$dsh_log" 2>/dev/null | tr -d ' ')
  if [ -z "$dsh_bytes" ]; then dsh_bytes=0; fi
  printf '%s' "$dsh_bytes"
}

dsh_now=$(dsh_size)
dsh_i=0
while [ "$dsh_i" -lt "$dsh_wait" ] && [ "$dsh_now" -le "$dsh_offset" ] && dsh_alive; do
  sleep 1
  dsh_i=$((dsh_i + 1))
  dsh_now=$(dsh_size)
done

dsh_data=$(tail -c "+$((dsh_offset + 1))" "$dsh_log" 2>/dev/null | base64 | tr -d '\n')
if dsh_alive; then dsh_living=true; else dsh_living=false; fi
printf '<<<DSH-JSON>>>{"offset":%s,"data":"%s","alive":%s}<<<DSH-END>>>' "$dsh_now" "$dsh_data" "$dsh_living"
"#;

/// Removes one directory tree. A path that is not there is already what the
/// caller wanted. The caller only ever aims this inside the host's own data
/// directory (ADR-0006).
const REMOVE_DIR: &str = r#"set -u
dsh_path=%%PATH%%
rm -rf -- "$dsh_path" || { printf '删不掉远端目录 %s\n' "$dsh_path" >&2; exit 2; }
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

/// Whether that machine's npm knows one version of the launcher's package.
///
/// A version published only as a GitHub `dsh-v*` tag is not on npm anywhere, and
/// the local installer forks to a source build for exactly that case. The
/// answer has to come from the machine that will do the installing, because its
/// registry is the one its install will read.
const NPM_HAS_VERSION: &str = r#"set -u
dsh_npm=$(command -v npm 2>/dev/null || true)
if [ -z "$dsh_npm" ]; then
  printf '这台机器上没有 npm\n' >&2
  exit 2
fi
dsh_said=$("$dsh_npm" view %%PACKAGE%% version %%REGISTRY%% 2>/dev/null | head -n 1 || true)
dsh_has=false
if [ -n "$dsh_said" ]; then dsh_has=true; fi
printf '<<<DSH-JSON>>>{"exists":%s}<<<DSH-END>>>' "$dsh_has"
"#;

/// Starts a process that outlives the ssh session (ADR-0007): its own session
/// (`setsid`), stdin from `/dev/null`, output appended to the remote log file,
/// and the pid printed for the remote running record.
///
/// The pid is `$!` as the shell saw it. Our sessions have no job control (bash
/// says so on stderr), so the background child is not a process-group leader and
/// `setsid` does not need to fork — `$!` is the process itself.
const SPAWN: &str = r#"set -u
dsh_log=%%LOG%%
mkdir -p "$(dirname "$dsh_log")" || exit 2
nohup setsid %%ARGV%% >> "$dsh_log" 2>&1 < /dev/null &
printf '<<<DSH-JSON>>>{"pid":%s}<<<DSH-END>>>' "$!"
"#;

/// Fills a script's slots. The values are already quoted by the caller; a value
/// is inserted once and never re-scanned, so a value that happens to spell
/// another slot cannot inject anything.
fn fill(script: &str, slots: &[(&str, String)]) -> String {
    let mut out = script.to_string();
    for (slot, value) in slots {
        out = out.replace(slot, value);
    }
    out
}

pub fn read_file_script(path: &str) -> String {
    fill(READ_FILE, &[("%%PATH%%", quote(path))])
}

pub fn write_file_script(path: &str, contents: &[u8]) -> String {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(contents);
    fill(
        WRITE_FILE,
        &[("%%PATH%%", quote(path)), ("%%BASE64%%", quote(&encoded))],
    )
}

pub fn list_dir_script(path: &str) -> String {
    fill(
        LIST_DIR,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATH%%", quote(path)),
        ],
    )
}

pub fn stop_pid_script(pid: i32) -> String {
    fill(STOP_PID, &[("%%PID%%", pid.to_string())])
}

pub fn stop_group_script(pid: i32) -> String {
    fill(STOP_GROUP, &[("%%PID%%", pid.to_string())])
}

pub fn alive_script(pid: i32) -> String {
    fill(ALIVE, &[("%%PID%%", pid.to_string())])
}

pub fn remove_dir_script(path: &str) -> String {
    fill(REMOVE_DIR, &[("%%PATH%%", quote(path))])
}

/// The script that asks whether that machine's npm knows one version. An empty
/// `registry` means "let that machine's own npm configuration decide".
pub fn npm_has_version_script(package: &str, registry: Option<&str>) -> String {
    let registry_arg = match registry.map(str::trim).filter(|r| !r.is_empty()) {
        Some(registry) => format!("--registry {}", quote(registry)),
        None => String::new(),
    };
    fill(
        NPM_HAS_VERSION,
        &[
            ("%%PACKAGE%%", quote(package)),
            ("%%REGISTRY%%", registry_arg),
        ],
    )
}

/// The tail read for one remote log. The values are numbers and a quoted path,
/// so nothing here can be read as shell syntax.
pub fn tail_script(log_path: &str, offset: u64, pid: i32, wait_secs: u64) -> String {
    fill(
        TAIL,
        &[
            ("%%LOG%%", quote(log_path)),
            ("%%OFFSET%%", offset.to_string()),
            ("%%PID%%", pid.to_string()),
            ("%%WAIT%%", wait_secs.to_string()),
        ],
    )
}

/// The size read for one remote log.
pub fn log_size_script(log_path: &str) -> String {
    fill(LOG_SIZE, &[("%%LOG%%", quote(log_path))])
}

/// The readiness read for one remote file.
pub fn file_ready_script(path: &str) -> String {
    fill(FILE_READY, &[("%%PATH%%", quote(path))])
}

/// The start-time read for one pid.
pub fn start_time_script(pid: i32) -> String {
    fill(START_TIME, &[("%%PID%%", pid.to_string())])
}

/// The occupancy probe for one port on that machine.
pub fn port_owner_script(port: u16) -> String {
    fill(PORT_OWNER, &[("%%PORT%%", port.to_string())])
}

/// The remote-side wait for one process tree to go.
pub fn wait_gone_script(pid: i32, wait_secs: u64) -> String {
    fill(
        WAIT_GONE,
        &[
            ("%%PID%%", pid.to_string()),
            ("%%WAIT%%", wait_secs.to_string()),
        ],
    )
}

/// The token exchange for one instance URL, made on that machine.
pub fn mint_script(url: &str) -> String {
    fill(MINT_REMOTE, &[("%%URL%%", quote(url))])
}

/// The script for one detached process: `argv` is the command line (already the
/// argv the launcher would run locally, never a shell string), `log_path` the
/// remote file its output is appended to.
pub fn spawn_script(argv: &[String], log_path: &str) -> String {
    let command = argv
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    fill(
        SPAWN,
        &[
            ("%%LOG%%", quote(log_path)),
            ("%%ARGV%%", command),
        ],
    )
}

/// The argv for a detached command whose **outcome** the launcher needs, run in
/// `cwd` (which is created if it is not there).
///
/// [`spawn_script`] detaches the process with `setsid` so it outlives the ssh
/// session — which also means it is not the launcher's child and nobody can
/// `wait` for it. So the command runs under a small `sh -c` wrapper that appends
/// its exit status to the same log the launcher is already reading
/// ([`EXIT_MARK`], read back by [`exit_status_of`]). A missing status line is
/// then meaningful in itself: the wrapper never got to write one, so the command
/// was killed rather than having exited.
///
/// `cwd` is where pnpm and git want to run: both read the tree they are working
/// on from the working directory, and `pnpm install --prefix` is not a
/// substitute for being in the directory for everything else pnpm resolves.
pub fn measured_argv(argv: &[String], cwd: &str) -> Vec<String> {
    let command = argv
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "mkdir -p {dir} && cd {dir} && {command}; printf '\\n{EXIT_MARK}%s{END}\\n' \"$?\"",
            dir = quote(cwd)
        ),
    ]
}

/// The exit status on one line of a log a [`measured_argv`] wrapper wrote, if
/// that line is the status line. `None` for every other line, and for text that
/// has no status line at all.
pub fn exit_status_of(line: &str) -> Option<i32> {
    let rest = line.trim().strip_prefix(EXIT_MARK)?;
    rest.strip_suffix(END)?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// The two machine actions that read something there
// ---------------------------------------------------------------------------

/// The shell text a terminal on this Mac runs on the other machine: land in the
/// instance's DSH_HOME with the same variables and PATH the local instance
/// terminal gets.
///
/// A local session is a `scripts/term_<id>.sh` file the launcher writes and the
/// terminal executes; there is nothing to write on another machine, so the
/// terminal is handed one command line instead ([`terminal_command`], which
/// carries this script over). The interpreter is the one the user gets in a
/// terminal on that machine — their login shell, on a pty.
pub fn terminal_script(home: &str, env: &[(String, String)], path_dirs: &[String]) -> String {
    let mut script = String::new();
    // The version's own `node_modules/.bin` first, so a bare `dsh` is the CLI
    // the launcher runs. 本机 writes a wrapper there for source checkouts; a
    // remote directory cannot be written to without asking, and pnpm's own
    // `.bin/dsh` needs no flag of ours — a directory that is not there costs
    // nothing in PATH.
    if !path_dirs.is_empty() {
        let joined = path_dirs
            .iter()
            .map(|dir| quote(dir))
            .collect::<Vec<_>>()
            .join(":");
        script.push_str(&format!("export PATH={joined}:\"$PATH\"\n"));
    }
    for (key, value) in env {
        script.push_str(&format!("export {key}={}\n", quote(value)));
    }
    script.push_str(&format!("cd {}\n", quote(home)));
    script.push_str("clear\n");
    script.push_str("exec \"${SHELL:-/bin/bash}\" -l\n");
    script
}

/// The one command line a terminal on this Mac is handed to open a session on
/// `target` that lands where the instance lives.
///
/// Two shells sit between the click and the session: the one inside the
/// terminal parses this line, and the login shell at the other end parses what
/// ssh hands it. The script is quoted for the first ([`quote`]),
/// so what ssh sends is the script's own text; the quoting *inside* the script —
/// every path and every value in it goes through [`quote`] — is what carries it
/// through the second. `-t` is what makes the far end a terminal session rather
/// than a one-shot command.
pub fn terminal_command(target: &str, script: &str) -> String {
    format!("ssh -t {target} {}", quote(script))
}

/// How much of a remote log the launcher copies home before opening it. A DSH
/// log grows without limit; the last stretch is the part anyone reads, and
/// bounding it keeps one click to a few hundred kilobytes regardless of how
/// long the instance has been printing.
pub const LOG_TAIL_BYTES: usize = 256 * 1024;

/// The last stretch of one remote log, as the machine handed it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogTail {
    /// The bytes at the end of the file, as text. Empty when the file is not
    /// there or holds nothing yet.
    pub text: String,
    /// Whether that machine has the file at all — an instance that has never
    /// printed anything has no log, which is a fact and not a failure.
    pub exists: bool,
}

/// Reads the last `max_bytes` of one file on that machine.
///
/// Not [`tail`]: that one follows a log **from an offset**, waiting there for
/// what a run appends next, and asking it for the end would mean reading the
/// size first and hoping nothing grows in between. Opening a log is a one-shot
/// question about a file that may be gigabytes long, so it is `tail -c` on the
/// far side — one call, and bounded no matter how big the file is.
pub async fn log_tail(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    log_path: &str,
    max_bytes: usize,
) -> Result<LogTail, HostError> {
    let out = transport
        .run(host, RemoteCall::new(log_tail_script(log_path, max_bytes)))
        .await?;
    decode_log_tail(&out)
}

/// The bounded tail read for one log file.
pub fn log_tail_script(log_path: &str, max_bytes: usize) -> String {
    fill(
        LOG_TAIL,
        &[
            ("%%LOG%%", quote(log_path)),
            ("%%BYTES%%", max_bytes.to_string()),
        ],
    )
}

/// The last stretch of one file, from its end. A file that is not there is an
/// empty answer **and** says so, so the caller can tell 「还没有日志」 from
/// 「日志里什么都还没写」.
///
/// The cut lands where the byte count says, which may be in the middle of a
/// line: this is a copy of a log for a human to read, not a file to parse.
const LOG_TAIL: &str = r#"set -u
dsh_log=%%LOG%%
dsh_bytes=%%BYTES%%
if [ -f "$dsh_log" ]; then
  dsh_there=true
  dsh_data=$(tail -c "$dsh_bytes" "$dsh_log" 2>/dev/null | base64 | tr -d '\n')
else
  dsh_there=false
  dsh_data=
fi
printf '<<<DSH-JSON>>>{"exists":%s,"data":"%s"}<<<DSH-END>>>' "$dsh_there" "$dsh_data"
"#;

pub fn decode_log_tail(out: &RemoteOutput) -> Result<LogTail, HostError> {
    #[derive(Deserialize)]
    struct Tail {
        exists: bool,
        data: String,
    }
    use base64::Engine as _;
    let tail: Tail = parse_marked(out)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(tail.data.as_bytes())
        .map_err(|e| HostError::remote_command(format!("远端日志无法解码：{e}")))?;
    Ok(LogTail {
        text: String::from_utf8_lossy(&bytes).to_string(),
        exists: tail.exists,
    })
}

// ---------------------------------------------------------------------------
// Answers of the non-JSON-shaped scripts
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Blob {
    data: String,
}

#[derive(Deserialize)]
struct Entries {
    entries: Vec<String>,
}

#[derive(Deserialize)]
struct Installed {
    versions: Vec<RemoteInstalledVersion>,
}

#[derive(Deserialize)]
struct Pid {
    pid: i32,
}

#[derive(Deserialize)]
struct Ack {
    ok: bool,
}

#[derive(Deserialize)]
struct HomePath {
    path: String,
}

#[derive(Deserialize)]
struct ProfileNames {
    profiles: Vec<String>,
}

pub fn decode_blob(out: &RemoteOutput) -> Result<Vec<u8>, HostError> {
    use base64::Engine as _;
    let blob: Blob = parse_marked(out)?;
    base64::engine::general_purpose::STANDARD
        .decode(blob.data.as_bytes())
        .map_err(|e| HostError::remote_command(format!("远端文件内容无法解码：{e}")))
}

pub fn decode_names(out: &RemoteOutput) -> Result<Vec<String>, HostError> {
    Ok(parse_marked::<Entries>(out)?.entries)
}

pub fn decode_installed_versions(
    out: &RemoteOutput,
) -> Result<Vec<RemoteInstalledVersion>, HostError> {
    Ok(parse_marked::<Installed>(out)?.versions)
}

pub fn decode_pid(out: &RemoteOutput) -> Result<i32, HostError> {
    Ok(parse_marked::<Pid>(out)?.pid)
}

pub fn decode_ack(out: &RemoteOutput) -> Result<(), HostError> {
    if parse_marked::<Ack>(out)?.ok {
        Ok(())
    } else {
        Err(HostError::remote_command("远端拒绝了这次写入"))
    }
}

/// The profile names one machine listed, in the same order the local one sorts
/// them: the two ways of reading one list must not disagree about it.
pub fn decode_profiles(out: &RemoteOutput) -> Result<Vec<String>, HostError> {
    let mut names = parse_marked::<ProfileNames>(out)?.profiles;
    names.sort();
    Ok(names)
}

#[derive(Deserialize)]
struct ProfilePackageRow {
    name: String,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Deserialize)]
struct ProfilePackages {
    #[serde(default)]
    manifest: Option<String>,
    #[serde(default)]
    installed: Vec<ProfilePackageRow>,
}

/// One profile's files out of the answer: the manifest's bytes and the installed
/// versions. A manifest that is not valid base64 reads as 「没有 manifest」 — the
/// same answer as a profile without one, because nothing can be projected out of
/// it either way — and a package whose version the script could not find is left
/// out rather than reported as an empty version.
pub fn decode_profile_packages(out: &RemoteOutput) -> Result<RemoteProfilePackages, HostError> {
    use base64::Engine as _;
    let answer: ProfilePackages = parse_marked(out)?;
    let manifest = answer
        .manifest
        .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).ok());
    let installed = answer
        .installed
        .into_iter()
        .filter_map(|row| match row.version {
            Some(version) if !version.is_empty() => Some((row.name, version)),
            _ => None,
        })
        .collect();
    Ok(RemoteProfilePackages {
        manifest,
        installed,
    })
}

// ---------------------------------------------------------------------------
// The in-memory double
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::ssh::{HostFuture, PortForward};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A payload wrapped the way the scripts print it, so the fake answers with
    /// the same shape the remote produces.
    pub(crate) fn marked(payload: &str) -> String {
        format!("{}{}{}", super::MARK, payload, super::END)
    }

    /// One call, as the fake recorded it. Tests assert on the script the
    /// launcher *would* have run, never on ssh or a real machine.
    ///
    /// Each forward call names the host it was aimed at: two instances on one
    /// machine must share a single connection, and that is only checkable when
    /// the calls say which machine they were for.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) enum FakeCall {
        Run(String),
        Spawn(String),
        StopPid(i32),
        StopGroup(i32),
        ReadFile(String),
        WriteFile(String, Vec<u8>),
        ListDir(String),
        OpenForward {
            host: RemoteHost,
            forward: PortForward,
        },
        CloseForward {
            host: RemoteHost,
            forward: PortForward,
        },
        CloseConnection(RemoteHost),
    }

    /// What the fake answers the next call with.
    #[derive(Clone, Debug)]
    pub(crate) enum FakeAnswer {
        Output {
            status: i32,
            stdout: String,
            stderr: String,
        },
        Pid(i32),
        Bytes(Vec<u8>),
        Names(Vec<String>),
        Done,
        Fail(HostError),
    }

    impl FakeAnswer {
        /// A script that answered with this (already marked) stdout.
        pub(crate) fn stdout(stdout: impl Into<String>) -> Self {
            Self::Output {
                status: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            }
        }

        /// A remote command that failed, with what it said on stderr.
        pub(crate) fn failed(status: i32, stderr: impl Into<String>) -> Self {
            Self::Output {
                status,
                stdout: String::new(),
                stderr: stderr.into(),
            }
        }

        /// The transport itself failed — a machine that is off, a key that is
        /// not accepted, a call that ran out of time.
        pub(crate) fn fail(kind: HostErrorKind, message: &str) -> Self {
            Self::Fail(HostError::new(kind, message))
        }
    }

    /// The in-memory [`HostTransport`]: what a test has instead of a remote
    /// machine. It records every call and answers from a queue, so a test can
    /// drive the page's states — reachable, unreachable, a machine without
    /// Node — without a server.
    pub(crate) struct FakeTransport {
        calls: Mutex<Vec<FakeCall>>,
        answers: Mutex<VecDeque<FakeAnswer>>,
    }

    impl FakeTransport {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                answers: Mutex::new(VecDeque::new()),
            })
        }

        /// Queues the answer the next call gets. Chainable so a test reads as
        /// one sentence.
        pub(crate) fn answering(self: &Arc<Self>, answer: FakeAnswer) -> Arc<Self> {
            self.answers.lock().unwrap().push_back(answer);
            self.clone()
        }

        pub(crate) fn calls(&self) -> Vec<FakeCall> {
            self.calls.lock().unwrap().clone()
        }

        /// The script of the last call — what a test looks at to check that the
        /// launcher asked the remote machine for the right thing.
        pub(crate) fn last_script(&self) -> String {
            match self.calls.lock().unwrap().last() {
                Some(FakeCall::Run(script))
                | Some(FakeCall::Spawn(script)) => script.clone(),
                _ => String::new(),
            }
        }

        fn next(&self) -> Result<FakeAnswer, HostError> {
            self.answers.lock().unwrap().pop_front().ok_or_else(|| {
                HostError::unreachable("测试没有给这次调用排好应答（FakeTransport）")
            })
        }

        /// The queued answer, or the transport failure it stands for: a test
        /// asking for 「这台机器连不上」 gets an `Err`, exactly as the real
        /// transport would produce one.
        fn answer(&self) -> Result<FakeAnswer, HostError> {
            match self.next()? {
                FakeAnswer::Fail(err) => Err(err),
                other => Ok(other),
            }
        }
    }

    fn wrong_shape(answer: &FakeAnswer) -> HostError {
        HostError::remote_command(format!("测试给这次调用排错了应答：{answer:?}"))
    }

    /// A transport that answers nothing: what the tests of modules that never
    /// touch a remote host (the local lifecycle, for one) construct `AppState`
    /// with.
    pub(crate) fn idle() -> Arc<dyn HostTransport> {
        FakeTransport::new()
    }

    /// The stdout a **successful** connection test prints — the sample every
    /// test that needs a reachable machine shares. It mirrors what
    /// [`connection_test_script`] builds, field for field.
    pub(crate) fn connection_stdout(
        node: Option<(&str, &str)>,
        pnpm: Option<(&str, &str)>,
    ) -> String {
        fn tool(value: Option<(&str, &str)>) -> String {
            match value {
                Some((path, version)) => {
                    format!("{{\"path\":\"{path}\",\"version\":\"{version}\"}}")
                }
                None => "null".to_string(),
            }
        }
        marked(&format!(
            "{{\"os\":\"Linux\",\"release\":\"6.8.0-45-generic\",\"arch\":\"x86_64\",\
             \"home\":\"/home/demo\",\"data_dir\":\"/home/demo/.local/share/dsh-launcher\",\
             \"data_dir_exists\":false,\"node\":{},\"pnpm\":{}}}",
            tool(node),
            tool(pnpm)
        ))
    }

    /// The stdout one file read answers with: the file's bytes, base64-encoded
    /// between the markers (the shape [`READ_FILE`] prints).
    pub(crate) fn file_stdout(contents: &[u8]) -> String {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(contents);
        marked(&format!("{{\"data\":\"{data}\"}}"))
    }

    /// The stdout one tail read answers with: the log's size now, the bytes that
    /// appeared since the previous offset (base64), and whether the process tree
    /// is still there.
    pub(crate) fn tail_stdout(text: &str, offset: u64) -> String {
        tail_stdout_with(text, offset, true)
    }

    /// The stdout the profile read answers with: the profile's `package.json`,
    /// base64-encoded, and one row per installed package. Mirrors what
    /// [`PROFILE_PACKAGES`] prints, field for field.
    pub(crate) fn profile_packages_stdout(manifest: &str, installed: &[(&str, &str)]) -> String {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(manifest.as_bytes());
        let rows: Vec<String> = installed
            .iter()
            .map(|(name, version)| format!("{{\"name\":\"{name}\",\"version\":\"{version}\"}}"))
            .collect();
        marked(&format!(
            "{{\"manifest\":\"{data}\",\"installed\":[{}]}}",
            rows.join(",")
        ))
    }

    /// The same read when the process is gone — the follower's signal that the
    /// instance ended on that machine.
    pub(crate) fn tail_stdout_after_exit(text: &str, offset: u64) -> String {
        tail_stdout_with(text, offset, false)
    }

    fn tail_stdout_with(text: &str, offset: u64, alive: bool) -> String {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        marked(&format!(
            "{{\"offset\":{offset},\"data\":\"{data}\",\"alive\":{alive}}}"
        ))
    }

    /// A tool as the environment probe reports it — one machine's answer for one
    /// of the two tools the launcher drives.
    pub(crate) fn remote_tool(path: &str, version: &str, source: &str) -> serde_json::Value {
        serde_json::json!({
            "path": path,
            "version": version,
            "source": source,
            "skipped": [],
        })
    }

    /// A tool the probe did not find, with what it passed over on the way.
    pub(crate) fn remote_tool_missing(
        skipped: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        serde_json::json!({
            "path": null,
            "version": null,
            "source": null,
            "skipped": skipped,
        })
    }

    /// One candidate the probe passed over.
    pub(crate) fn skipped(
        path: &str,
        version: &str,
        reason: &str,
        major: Option<u64>,
    ) -> serde_json::Value {
        serde_json::json!({
            "path": path,
            "version": version,
            "reason": reason,
            "major": major,
        })
    }

    /// The stdout a **successful** environment probe prints — what the fake
    /// answers the launcher with instead of a real machine's `bash -ilc`.
    pub(crate) fn environment_stdout(
        node: serde_json::Value,
        pnpm: serde_json::Value,
        managers: &[&str],
    ) -> String {
        marked(
            &serde_json::json!({
                "shell": "/bin/bash",
                "managers": managers,
                "node": node,
                "pnpm": pnpm,
            })
            .to_string(),
        )
    }

    impl HostTransport for FakeTransport {
        fn run<'a>(
            &'a self,
            _host: &'a RemoteHost,
            call: RemoteCall,
        ) -> HostFuture<'a, Result<RemoteOutput, HostError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(FakeCall::Run(call.script));
                match self.answer()? {
                    FakeAnswer::Output {
                        status,
                        stdout,
                        stderr,
                    } => Ok(RemoteOutput {
                        status,
                        stdout,
                        stderr,
                    }),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn spawn_detached<'a>(
            &'a self,
            _host: &'a RemoteHost,
            argv: &'a [String],
            log_path: &'a str,
        ) -> HostFuture<'a, Result<i32, HostError>> {
            let script = spawn_script(argv, log_path);
            Box::pin(async move {
                self.calls.lock().unwrap().push(FakeCall::Spawn(script));
                match self.answer()? {
                    FakeAnswer::Pid(pid) => Ok(pid),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn stop_pid<'a>(
            &'a self,
            _host: &'a RemoteHost,
            pid: i32,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(FakeCall::StopPid(pid));
                match self.answer()? {
                    FakeAnswer::Done => Ok(()),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn stop_group<'a>(
            &'a self,
            _host: &'a RemoteHost,
            pid: i32,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(FakeCall::StopGroup(pid));
                match self.answer()? {
                    FakeAnswer::Done => Ok(()),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn read_file<'a>(
            &'a self,
            _host: &'a RemoteHost,
            path: &'a str,
        ) -> HostFuture<'a, Result<Vec<u8>, HostError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(FakeCall::ReadFile(path.to_string()));
                match self.answer()? {
                    FakeAnswer::Bytes(bytes) => Ok(bytes),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn write_file<'a>(
            &'a self,
            _host: &'a RemoteHost,
            path: &'a str,
            contents: Vec<u8>,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(FakeCall::WriteFile(path.to_string(), contents));
                match self.answer()? {
                    FakeAnswer::Done => Ok(()),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn list_dir<'a>(
            &'a self,
            _host: &'a RemoteHost,
            path: &'a str,
        ) -> HostFuture<'a, Result<Vec<String>, HostError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(FakeCall::ListDir(path.to_string()));
                match self.answer()? {
                    FakeAnswer::Names(names) => Ok(names),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn open_forward<'a>(
            &'a self,
            host: &'a RemoteHost,
            forward: PortForward,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(FakeCall::OpenForward {
                    host: host.clone(),
                    forward,
                });
                match self.answer()? {
                    FakeAnswer::Done => Ok(()),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn close_forward<'a>(
            &'a self,
            host: &'a RemoteHost,
            forward: PortForward,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(FakeCall::CloseForward {
                    host: host.clone(),
                    forward,
                });
                match self.answer()? {
                    FakeAnswer::Done => Ok(()),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn close_connection<'a>(
            &'a self,
            host: &'a RemoteHost,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(FakeCall::CloseConnection(host.clone()));
                match self.answer()? {
                    FakeAnswer::Done => Ok(()),
                    other => Err(wrong_shape(&other)),
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{
        connection_stdout, environment_stdout, marked, remote_tool, remote_tool_missing, skipped,
        FakeAnswer, FakeCall, FakeTransport,
    };
    use super::*;
    use crate::ssh::{HostTransport, SshTransport};
    use base64::Engine as _;
    use std::path::{Path, PathBuf};

    const TARGET: &str = "zjq_aliyun";

    fn host() -> RemoteHost {
        RemoteHost {
            id: "host-lab".into(),
            target: TARGET.into(),
            socket: PathBuf::from("/tmp/dsh-launcher-test.sock"),
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// Runs one script on **this** machine's shell, in the same shape the
    /// transport runs it on a remote one. The scripts know nothing about which
    /// machine they are on, so what a real shell prints here is what a real
    /// shell prints there — the only thing in the suite that exercises the
    /// shell text itself.
    fn local_shell(script: &str) -> RemoteOutput {
        let out = std::process::Command::new("/bin/bash")
            .arg("-ilc")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        RemoteOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    /// The happy path: the machine answers, and the page gets its system, its
    /// home, its data directory and both tools.
    #[test]
    fn the_connection_test_reports_what_the_machine_is() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(connection_stdout(
            Some(("/home/linuxbrew/.linuxbrew/bin/node", "v26.0.0")),
            Some(("/home/demo/.local/share/pnpm/pnpm", "11.1.2")),
        )));
        let transport: &dyn HostTransport = &*fake;

        let test = block_on(connection_test(transport, &host(), None));

        assert!(test.ok, "{:?}", test.message);
        let system = test.system.unwrap();
        assert_eq!(system.os, "Linux");
        assert_eq!(system.arch, "x86_64");
        assert_eq!(test.home.as_deref(), Some("/home/demo"));
        assert_eq!(
            test.data_dir.as_deref(),
            Some("/home/demo/.local/share/dsh-launcher")
        );
        assert!(!test.data_dir_exists, "默认数据目录一开始并不存在");
        assert_eq!(
            test.node.as_ref().map(|t| t.version.as_str()),
            Some("v26.0.0")
        );
        assert_eq!(test.pnpm.as_ref().map(|t| t.path.as_str()), Some("/home/demo/.local/share/pnpm/pnpm"));
    }

    /// A machine without Node or pnpm is still a machine worth connecting to;
    /// what it lacks is reported as missing, not as a failure and not as a
    /// plausible-looking version.
    #[test]
    fn a_machine_missing_node_or_pnpm_says_so() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(connection_stdout(
            Some(("/usr/bin/node", "v18.19.1")),
            None,
        )));
        let transport: &dyn HostTransport = &*fake;

        let test = block_on(connection_test(transport, &host(), None));

        assert!(test.ok);
        assert!(test.node.is_some());
        assert!(test.pnpm.is_none(), "没有 pnpm 就报没有");
        assert_eq!(test.node.unwrap().version, "v18.19.1");
    }

    /// Every one of the four classes has to reach the page intact: the class,
    /// and a sentence the user can act on.
    #[test]
    fn the_four_failure_classes_reach_the_page() {
        let cases = [
            (
                HostErrorKind::Unreachable,
                "ssh: connect to host lab port 22: Connection refused",
                "连不上",
            ),
            (
                HostErrorKind::Auth,
                "Permission denied (publickey).",
                "认证失败",
            ),
            (HostErrorKind::Timeout, "ssh 在 30 秒内没有返回", "超时"),
            (
                HostErrorKind::RemoteCommand,
                "远端命令失败（退出码 127）：base64: not found",
                "远端命令失败",
            ),
        ];
        for (kind, message, label) in cases {
            let fake =
                FakeTransport::new().answering(FakeAnswer::fail(kind, message));
            let transport: &dyn HostTransport = &*fake;

            let test = block_on(connection_test(transport, &host(), None));

            assert!(!test.ok);
            assert_eq!(test.kind, Some(kind));
            let said = test.message.unwrap();
            assert!(said.contains(message), "{said}");
            assert_eq!(kind.label(), label);
            assert!(test.system.is_none(), "失败时不该报机器的结论");
        }
    }

    /// A remote script that fails — a missing file, a command that is not
    /// there — carries the tail of its stderr back, and it is the class
    /// 「远端命令失败」, not a connection problem.
    #[test]
    fn a_failed_remote_script_reports_the_tail_of_its_stderr() {
        let fake = FakeTransport::new().answering(FakeAnswer::failed(
            127,
            "bash: no job control in this shell\nbash: line 3: pnpm: command not found\n",
        ));
        let transport: &dyn HostTransport = &*fake;

        let test = block_on(connection_test(transport, &host(), None));

        assert_eq!(test.kind, Some(HostErrorKind::RemoteCommand));
        let said = test.message.unwrap();
        assert!(said.contains("pnpm: command not found"), "{said}");
        assert!(said.contains("退出码 127"), "{said}");
        assert!(!said.contains("no job control"), "登录 shell 的废话不是失败原因：{said}");
    }

    /// The configured data directory reaches the script as one quoted word, and
    /// a path with a space or a quote in it stays one word.
    #[test]
    fn the_data_directory_reaches_the_script_quoted() {
        let script = connection_test_script(Some("/srv/my launcher/it's here"));
        assert!(
            script.contains("dsh_configured='/srv/my launcher/it'\\''s here'"),
            "{script}"
        );
        assert!(
            connection_test_script(None).contains("dsh_configured=\n"),
            "没有配置时留空，让那台机器自己选默认值"
        );
        // The scripts print what the parsing side looks for: one pair of markers,
        // one place they are defined (script text and parser must not drift).
        assert!(
            script.contains(MARK) && script.contains(END),
            "脚本必须用与解析端同一对标记：{script}"
        );
    }

    /// The escaping, end to end: the script a call site builds, run by a real
    /// shell, must hand back the original path. This is the bug the spec calls
    /// out as the most likely one — ssh goes through two shells.
    #[test]
    fn a_path_with_spaces_and_quotes_survives_a_real_shell() {
        let path = "/home/me/a b/it's 'quoted'/file.txt";
        let script = read_file_script(path);

        // Only the line that names the path is exercised: running the whole
        // script locally would read the local filesystem.
        let assignment = script
            .lines()
            .find(|line| line.starts_with("dsh_path="))
            .unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("{assignment}\nprintf %s \"$dsh_path\""))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), path);

        // And the write script puts its payload in the same shape.
        let write = write_file_script(path, b"hello");
        let assignment = write
            .lines()
            .find(|line| line.starts_with("dsh_path="))
            .unwrap();
        assert!(assignment.ends_with("'/home/me/a b/it'\\''s '\\''quoted'\\''/file.txt'"));
    }

    /// The script, run by a **real** shell on this machine, answers what the
    /// parsing side expects. The samples the fake serves were written from this
    /// shape; this is the test that keeps the script and the parser from
    /// drifting, and the only thing in the suite that exercises the shell text
    /// itself (the machine answering happens to be this Mac, which the script
    /// knows nothing about).
    #[test]
    fn the_connection_test_script_answers_parsable_json_on_a_real_shell() {
        let output = local_shell(&connection_test_script(None));

        let facts: Facts = parse_marked(&output).expect("真实 shell 跑出来的答案要能解析");

        assert!(!facts.os.is_empty() && !facts.release.is_empty() && !facts.arch.is_empty());
        assert_eq!(facts.home, std::env::var("HOME").unwrap_or_default());
        assert!(
            facts.data_dir.ends_with("/.local/share/dsh-launcher"),
            "默认数据目录按那台机器的家目录推：{}",
            facts.data_dir
        );
        assert_eq!(
            facts.data_dir_exists,
            std::path::Path::new(&facts.data_dir).is_dir(),
            "「目录存不存在」要如实回答"
        );
        for tool in [facts.node, facts.pnpm].into_iter().flatten() {
            assert!(
                tool.path.starts_with('/'),
                "报出来的路径必须是绝对路径（执行时要用它）：{}",
                tool.path
            );
        }
    }

    /// What a machine has under its versions directory, told the way the page
    /// needs it: one row per version, with its directory and whether the bin is
    /// ready. A half-installed version is not something anyone can start, and
    /// the machine that owns the directory is the only one that can say so.
    #[test]
    fn the_installed_versions_script_answers_each_versions_path_and_bin_readiness() {
        let scratch = std::env::temp_dir().join(format!("dsh-versions-{}", uuid::Uuid::new_v4()));
        let versions = scratch.join("versions");
        // An npm tree, the layout an installed release has...
        let npm_tree = versions.join("0.1.0-rc.6");
        std::fs::create_dir_all(npm_tree.join("node_modules/@deepseek-ai/dsh/lib")).unwrap();
        std::fs::write(
            npm_tree.join("node_modules/@deepseek-ai/dsh/lib/bin.js"),
            "// dsh\n",
        )
        .unwrap();
        // ...a repo checkout, the other layout...
        let checkout = versions.join("0.1.0-rc.5");
        std::fs::create_dir_all(checkout.join("apps/cli/lib")).unwrap();
        std::fs::write(checkout.join("apps/cli/package.json"), "{}").unwrap();
        std::fs::write(checkout.join("apps/cli/lib/bin.js"), "// dsh\n").unwrap();
        // ...and an install that stopped before its bin existed.
        std::fs::create_dir_all(versions.join("0.1.0-rc.4")).unwrap();

        let out = local_shell(&installed_versions_script(&versions.to_string_lossy()));
        let found = decode_installed_versions(&out).expect("真实 shell 跑出来的答案要能解析");

        let by_name = |name: &str| found.iter().find(|v| v.name == name).unwrap().clone();
        assert_eq!(found.len(), 3);
        assert_eq!(by_name("0.1.0-rc.6").dir, npm_tree.to_string_lossy());
        assert!(by_name("0.1.0-rc.6").bin_ready, "装好的版本 bin 就绪");
        assert_eq!(by_name("0.1.0-rc.5").dir, checkout.to_string_lossy());
        assert!(by_name("0.1.0-rc.5").bin_ready, "repo checkout 也是就绪");
        assert!(!by_name("0.1.0-rc.4").bin_ready, "bin 没写出来就不是就绪");

        std::fs::remove_dir_all(&scratch).ok();
    }

    /// The remote bin check is the local launcher's rule applied on that
    /// machine, and the two must name the same files: a version the machine
    /// calls ready has to be one the launcher could start there.
    #[test]
    fn the_remote_bin_check_uses_the_local_launchers_two_layouts() {
        let script = installed_versions_script("/opt/dsh/versions");
        let scratch = std::env::temp_dir().join(format!("dsh-layout-{}", uuid::Uuid::new_v4()));

        let npm_tree = scratch.join("versions/1.0.0");
        std::fs::create_dir_all(&npm_tree).unwrap();
        assert_eq!(
            crate::launch::version_bin(&npm_tree),
            npm_tree.join("node_modules/@deepseek-ai/dsh/lib/bin.js")
        );
        assert!(
            script.contains("$dsh_entry/node_modules/@deepseek-ai/dsh/lib/bin.js"),
            "{script}"
        );

        let checkout = scratch.join("versions/1.0.1");
        std::fs::create_dir_all(checkout.join("apps/cli")).unwrap();
        std::fs::write(checkout.join("apps/cli/package.json"), "{}").unwrap();
        assert_eq!(
            crate::launch::version_bin(&checkout),
            checkout.join("apps/cli/lib/bin.js")
        );
        assert!(
            script.contains("$dsh_entry/apps/cli/lib/bin.js")
                && script.contains("$dsh_entry/apps/cli/package.json"),
            "{script}"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    /// The registry a host record names is a fact about *that* machine's npm:
    /// it has to reach the script as one quoted word.
    #[test]
    fn the_versions_query_reaches_the_machine_quoted() {
        let script = available_versions_script("https://npm.example.com/it's mirror");
        assert!(
            script.contains("--registry 'https://npm.example.com/it'\\''s mirror'"),
            "{script}"
        );
    }

    /// npm's two listings come back as the text the local path reads, so one
    /// conversion in the caller serves both sources.
    #[test]
    fn the_versions_query_answers_both_npm_listings() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(
            "{\"versions\":\"[\\\"0.1.0-rc.6\\\"]\",\"time\":\"{\\\"0.1.0-rc.6\\\":\\\"2026-08-01T12:00:00Z\\\"}\"}",
        )));
        let transport: &dyn HostTransport = &*fake;

        let listings = block_on(available_versions(
            transport,
            &host(),
            "https://npm.example.com",
        ))
        .unwrap();

        assert_eq!(listings.versions, "[\"0.1.0-rc.6\"]");
        assert!(listings.time.contains("2026-08-01"), "{}", listings.time);
    }

    /// A machine that is not there says so — the page must not be handed a
    /// plausible-looking empty list instead.
    #[test]
    fn an_unreachable_machine_fails_both_queries() {
        for answer in [
            FakeAnswer::fail(HostErrorKind::Unreachable, "连接被拒绝"),
            FakeAnswer::failed(2, "npm: command not found"),
        ] {
            let fake = FakeTransport::new().answering(answer);
            let transport: &dyn HostTransport = &*fake;

            let versions = block_on(installed_versions(transport, &host(), "/opt/dsh/versions"));
            let available = block_on(available_versions(transport, &host(), "https://npm.example.com"));

            assert!(versions.is_err(), "{versions:?}");
            assert!(available.is_err(), "{available:?}");
        }
    }

    /// The transport's other capabilities: each one is the same plumbing, and
    /// each one has to reach the remote machine with the right request. The
    /// callers (installs, profiles, lifecycle, the tunnel) are later tickets'
    /// work — this only keeps the seam honest.
    #[test]
    fn the_other_transport_capabilities_ask_for_the_right_thing() {
        let fake = FakeTransport::new()
            .answering(FakeAnswer::Bytes(b"version: 1\n".to_vec()))
            .answering(FakeAnswer::Names(vec!["web".into(), "lab".into()]))
            .answering(FakeAnswer::Pid(4242))
            .answering(FakeAnswer::Done)
            .answering(FakeAnswer::Done);
        let transport: &dyn HostTransport = &*fake;
        let host = host();

        assert_eq!(
            block_on(transport.read_file(&host, "/home/demo/package.json")).unwrap(),
            b"version: 1\n"
        );
        assert_eq!(
            block_on(transport.list_dir(&host, "/home/demo/.dsh")).unwrap(),
            vec!["web".to_string(), "lab".to_string()]
        );
        assert_eq!(
            block_on(transport.spawn_detached(
                &host,
                &["/usr/bin/node".to_string(), "/opt/dsh/bin.js".to_string()],
                "/home/demo/.local/share/dsh-launcher/logs/i-1.log",
            ))
            .unwrap(),
            4242
        );
        block_on(transport.stop_pid(&host, 4242)).unwrap();
        block_on(transport.write_file(&host, "/home/demo/.dsh/icons/i-1.png", vec![1, 2, 3]))
            .unwrap();

        let calls = fake.calls();
        assert_eq!(calls.len(), 5);
        let spawned = match &calls[2] {
            FakeCall::Spawn(script) => script.clone(),
            other => panic!("排错了：{other:?}"),
        };
        assert!(spawned.contains("nohup setsid"), "{spawned}");
        assert!(spawned.contains("< /dev/null"), "脱离会话的进程要断掉 stdin：{spawned}");
        assert!(
            spawned.contains("'/usr/bin/node' '/opt/dsh/bin.js'"),
            "argv 逐段引用，调用处不拼 shell 字符串：{spawned}"
        );
        assert_eq!(calls[3], FakeCall::StopPid(4242));
        assert_eq!(
            calls[4],
            FakeCall::WriteFile(
                "/home/demo/.dsh/icons/i-1.png".into(),
                vec![1, 2, 3]
            )
        );
    }

    /// The real transport is the one that builds the ssh command line, and the
    /// script it carries is the script from this module — nothing here is a
    /// stub.
    #[test]
    fn the_real_transport_would_dial_with_the_script_built_here() {
        let host = host();
        let script = connection_test_script(None);
        let argv = crate::ssh::ssh_argv(
            &host.target,
            &host.socket,
            &crate::ssh::script_command(&script),
        );
        assert_eq!(argv.last().unwrap(), &quote(&script));
        assert_eq!(argv[argv.len() - 3], crate::ssh::REMOTE_SHELL);
        assert_eq!(argv[argv.len() - 2], "-ilc");
        let _ = SshTransport::new();
    }

    // -----------------------------------------------------------------------
    // The environment probe
    // -----------------------------------------------------------------------

    /// A fixture tool: a script that prints the version it was made with, so the
    /// candidate walk can be driven against a controlled filesystem.
    fn fake_tool(home: &Path, rel: &str, version: &str) -> PathBuf {
        let exe = home.join(rel);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, format!("#!/bin/sh\necho {version}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        exe
    }

    /// A `PATH` a fixture can own: the tools the probe itself uses (`sed`, `tr`,
    /// `head`, `cut`, `sort`, `ls`), so a fixture machine can be described
    /// without also describing this Mac's PATH — and without a real `node`
    /// sneaking into the answer.
    fn fixture_tools(home: &Path) -> PathBuf {
        let dir = home.join("tools");
        std::fs::create_dir_all(&dir).unwrap();
        for tool in ["sed", "tr", "head", "cut", "sort", "ls"] {
            let source = ["/usr/bin", "/bin"]
                .iter()
                .map(|prefix| Path::new(prefix).join(tool))
                .find(|candidate| candidate.is_file());
            if let Some(source) = source {
                let link = dir.join(tool);
                if !link.exists() {
                    std::os::unix::fs::symlink(source, link).unwrap();
                }
            }
        }
        dir
    }

    fn probe_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    /// Runs the probe script in a real shell and parses its answer.
    ///
    /// `-c` rather than the `-ilc` the launcher dials with: what is under test is
    /// the walk over the candidate list, and the fixtures have to own `PATH` and
    /// `HOME` for the answer to be about them instead of about this Mac. The
    /// interactive login shape is pinned by `script_command` and by the
    /// connection test.
    fn run_probe(
        home: &Path,
        path: &str,
        manual_node: Option<&str>,
        fallback: &[String],
    ) -> RemoteEnvironment {
        let dirs: Vec<&str> = fallback.iter().map(|dir| dir.as_str()).collect();
        let script = environment_script_with(manual_node, None, &dirs);
        let out = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(&script)
            // Cleared, so the probe answers for the fixture machine and nothing
            // else: an inherited `BASH_ENV` or locale could rewrite PATH from
            // under the fixture.
            .env_clear()
            .env("HOME", home)
            .env("PATH", path)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let output = RemoteOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        };
        parse_marked(&output).unwrap_or_else(|e| panic!("探测脚本的答案要能解析：{e}\n{output:?}"))
    }

    fn dirs_of(home: &Path, rels: &[&str]) -> Vec<String> {
        rels.iter()
            .map(|rel| home.join(rel).to_string_lossy().into_owned())
            .collect()
    }

    /// The install directories are the Linux set, in the order they are tried,
    /// and the probe walks exactly that list (ADR-0005 追加 4).
    #[test]
    fn the_remote_install_directories_are_the_linux_set_in_order() {
        let dirs = fallback_bins();
        assert_eq!(dirs[0], "/home/linuxbrew/.linuxbrew/bin");
        assert!(dirs.contains(&"$HOME/.linuxbrew/bin"));
        assert!(dirs.contains(&"$HOME/.nvm/versions/node/*/bin"));
        assert!(dirs.contains(&"$HOME/.local/share/fnm/node-versions/*/installation/bin"));
        assert!(dirs.contains(&"$HOME/.local/share/pnpm"));
        assert!(dirs.contains(&"$HOME/.local/bin"));
        assert_eq!(dirs.last().unwrap(), &"/usr/local/bin");
        assert!(
            !dirs
                .iter()
                .any(|dir| dir.contains("Library/pnpm") || dir.contains("/opt/homebrew")),
            "本机的目录不该出现在远程候选里：{dirs:?}"
        );

        // The list is what the script walks, in that order: every entry has to
        // appear after the one before it.
        let script = environment_script(None, None);
        let mut cursor = 0;
        for dir in &dirs {
            let at = script[cursor..]
                .find(dir)
                .map(|i| i + cursor)
                .unwrap_or_else(|| panic!("脚本里没有 {dir}，或者它排在前一个候选之前"));
            cursor = at + dir.len();
        }
    }

    /// The three sources in their fixed order: what the user picked, then the
    /// directories this interactive login shell has on PATH, then the install
    /// directories.
    #[test]
    fn the_probe_takes_the_pick_then_the_path_then_the_install_directories() {
        let home = probe_home("probe-order");
        let from_path = fake_tool(&home, "path/bin/node", "v22.1.0");
        fake_tool(&home, "fallback/bin/node", "v24.0.0");
        let picked = fake_tool(&home, "picked/node", "v20.0.0");
        let path = format!(
            "{}:{}",
            fixture_tools(&home).display(),
            home.join("path/bin").display()
        );
        let fallback = dirs_of(&home, &["fallback/bin"]);

        let env = run_probe(&home, &path, None, &fallback);
        assert_eq!(env.node.path.as_deref(), Some(from_path.to_str().unwrap()));
        assert_eq!(env.node.source.as_deref(), Some("system"));
        assert!(env.node.skipped.is_empty(), "没有低于门槛的候选就不该报跳过");

        let env = run_probe(&home, &path, Some(picked.to_str().unwrap()), &fallback);
        assert_eq!(
            env.node.path.as_deref(),
            Some(picked.to_str().unwrap()),
            "用户指定的排在系统环境变量前面"
        );
        assert_eq!(env.node.source.as_deref(), Some("manual"));
    }

    /// The remote-only rule: a Node below the recommended major is passed over,
    /// the walk continues, and the page is told which one it was and why. Only
    /// when nothing qualifies is an older one used (ADR-0005 追加 2).
    #[test]
    fn the_probe_passes_over_an_old_node_and_names_it() {
        let home = probe_home("probe-skip");
        let old = fake_tool(&home, "apt/bin/node", "v18.19.1");
        let good = fake_tool(&home, "modern/bin/node", "v22.14.0");
        let path = format!(
            "{}:{}:{}",
            fixture_tools(&home).display(),
            home.join("apt/bin").display(),
            home.join("modern/bin").display()
        );
        let fallback = dirs_of(&home, &["none/bin"]);

        let env = run_probe(&home, &path, None, &fallback);
        assert_eq!(env.node.path.as_deref(), Some(good.to_str().unwrap()));
        assert_eq!(env.node.source.as_deref(), Some("system"));
        assert_eq!(env.node.skipped.len(), 1);
        assert_eq!(env.node.skipped[0].path, old.to_string_lossy());
        assert_eq!(env.node.skipped[0].version, "v18.19.1");
        assert_eq!(env.node.skipped[0].reason, "below-minimum");
        assert_eq!(env.node.skipped[0].major, Some(18));

        // Nothing qualifies: the last candidate below the floor is still used —
        // a machine with only an old Node can still be looked at — and both are
        // reported as passed over.
        let older = fake_tool(&home, "older/bin/node", "v16.20.2");
        let path = format!(
            "{}:{}:{}",
            fixture_tools(&home).display(),
            home.join("apt/bin").display(),
            home.join("older/bin").display()
        );
        let env = run_probe(&home, &path, None, &[]);
        assert_eq!(
            env.node.path.as_deref(),
            Some(older.to_str().unwrap()),
            "没有合格候选时用最后那个不合格的，而不是最早那个"
        );
        assert_eq!(env.node.skipped.len(), 2, "两个低于门槛的候选都要报出来");
    }

    /// pnpm is not driven below its floor or with a version nobody can read, and
    /// it has no last resort: a pnpm the launcher cannot reason about drives
    /// nothing, so it is reported and then simply not found.
    #[test]
    fn the_probe_passes_over_a_pnpm_it_cannot_drive() {
        let home = probe_home("probe-pnpm");
        let old = fake_tool(&home, "bin/pnpm", "9.15.0");
        let path = format!(
            "{}:{}",
            fixture_tools(&home).display(),
            home.join("bin").display()
        );

        let env = run_probe(&home, &path, None, &[]);
        assert!(env.pnpm.path.is_none(), "低版本的 pnpm 不能被选中");
        assert_eq!(env.pnpm.skipped.len(), 1);
        assert_eq!(env.pnpm.skipped[0].path, old.to_string_lossy());
        assert_eq!(env.pnpm.skipped[0].reason, "below-minimum");
        assert_eq!(env.pnpm.skipped[0].major, Some(9));

        let home = probe_home("probe-pnpm-odd");
        fake_tool(&home, "bin/pnpm", "not-a-version");
        let path = format!(
            "{}:{}",
            fixture_tools(&home).display(),
            home.join("bin").display()
        );
        let env = run_probe(&home, &path, None, &[]);
        assert!(env.pnpm.path.is_none());
        assert_eq!(env.pnpm.skipped[0].reason, "unparsable");
    }

    /// A hand-typed path that is not there is reported as passed over, and the
    /// walk carries on: the user's path is not a reason to leave a machine that
    /// has a working tool with none.
    #[test]
    fn an_unusable_pick_is_reported_and_the_walk_goes_on() {
        let home = probe_home("probe-unusable");
        let good = fake_tool(&home, "bin/node", "v22.1.0");
        let missing = home.join("gone/node");
        let path = format!(
            "{}:{}",
            fixture_tools(&home).display(),
            home.join("bin").display()
        );

        let env = run_probe(&home, &path, Some(missing.to_str().unwrap()), &[]);

        assert_eq!(env.node.path.as_deref(), Some(good.to_str().unwrap()));
        assert_eq!(env.node.source.as_deref(), Some("system"));
        assert_eq!(env.node.skipped.len(), 1);
        assert_eq!(env.node.skipped[0].path, missing.to_string_lossy());
        assert_eq!(env.node.skipped[0].reason, "unusable");
    }

    /// The shell and the managers are that machine's, read in the one shell the
    /// answer comes from.
    #[test]
    fn the_probe_reports_the_shell_and_managers_of_that_machine() {
        let home = probe_home("probe-facts");
        std::fs::create_dir_all(home.join(".nvm")).unwrap();
        let path = fixture_tools(&home).to_string_lossy().into_owned();

        let env = run_probe(&home, &path, None, &[]);

        assert_eq!(env.shell, "/bin/bash");
        assert!(
            env.managers.iter().any(|m| m == "nvm"),
            "装了 nvm 就要报出来，安装引导才能顺着它给命令：{:?}",
            env.managers
        );
    }

    /// The whole probe is one call, and the launcher reads its answer: the
    /// sources, the managers and what was passed over all travel in it.
    #[test]
    fn the_probe_is_one_interactive_login_shell_and_its_answer_is_read() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(environment_stdout(
            remote_tool("/usr/local/bin/node", "v22.14.0", "system"),
            remote_tool_missing(vec![skipped(
                "/usr/bin/pnpm",
                "9.15.0",
                "below-minimum",
                Some(9),
            )]),
            &["nvm"],
        )));
        let transport: &dyn HostTransport = &*fake;

        let env = block_on(environment(transport, &host(), Some("/opt/pick/node"), None)).unwrap();

        assert_eq!(env.shell, "/bin/bash");
        assert_eq!(env.node.source.as_deref(), Some("system"));
        assert_eq!(env.pnpm.path, None);
        assert_eq!(env.pnpm.skipped[0].reason, "below-minimum");
        assert_eq!(env.managers, vec!["nvm".to_string()]);

        // The pick and the candidate list travel in the script, so the machine
        // that answered is the machine that was asked — and it was asked to walk
        // the Linux install directories, not this Mac's.
        let script = fake.last_script();
        assert!(script.contains("'/opt/pick/node'"), "{script}");
        assert!(script.contains("$HOME/.nvm/versions/node/*/bin"), "{script}");
        assert!(script.contains("$HOME/.local/share/pnpm"), "{script}");
    }

    /// A remote pick is validated by running `--version` **there**: the launcher
    /// cannot execute a file on another computer, so the script carries the path
    /// the user typed and the answer says whether it is there and what it said.
    #[test]
    fn a_remote_pick_is_validated_by_running_it_on_that_machine() {
        let cases = [
            (
                r#"{"exists":true,"version":"v22.14.0"}"#,
                ManualProbe::Version("v22.14.0".to_string()),
            ),
            (r#"{"exists":true,"version":null}"#, ManualProbe::Unusable),
            (r#"{"exists":false,"version":null}"#, ManualProbe::Missing),
        ];
        for (answer, expected) in cases {
            let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(answer)));
            let transport: &dyn HostTransport = &*fake;

            let probed =
                block_on(probe_manual_tool(transport, &host(), "/opt/node/bin/node")).unwrap();

            assert_eq!(probed, expected, "答案：{answer}");
            let script = fake.last_script();
            assert!(
                script.contains("'/opt/node/bin/node'"),
                "要真的在那台机器上跑这个路径：{script}"
            );
            assert!(script.contains("--version"), "校验方式就是跑一次 --version：{script}");
        }
    }

    /// The install guidance is one command the user runs on **this** Mac, and it
    /// has to survive both shells on the way to the machine: the user's shell
    /// hands ssh its arguments, ssh joins them, and the remote shell splits them
    /// again. Both shells are real here, and the recipe is chosen so the result
    /// is visible — a broken quoting shows up as the wrong output.
    #[test]
    fn an_install_command_survives_both_shells_on_the_way_to_the_machine() {
        let command = ssh_command("lab", "echo one two | tr a-z A-Z");
        assert!(command.starts_with("ssh lab "), "{command}");
        assert!(
            ssh_command("user@example.com -p 2222", "echo hi")
                .starts_with("ssh user@example.com -p 2222 "),
            "自由目标（带自己的参数）也要能用"
        );

        // What the user's shell hands ssh.
        let handed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s\\n' {command}"))
            .output()
            .unwrap();
        let words: Vec<String> = String::from_utf8_lossy(&handed.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(words[0], "ssh");
        assert_eq!(words[1], "lab");
        assert_eq!(words.len(), 3, "ssh 只该拿到一个参数：{words:?}");

        // What ssh hands the remote shell: its arguments joined with spaces.
        // Running that in a real shell is what the machine does with it.
        let remote = words[2..].join(" ");
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&remote)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "ONE TWO",
            "远端 shell 解析出来的命令是：{remote}"
        );
    }

    /// The same round trip when the recipe quotes a path of its own — what
    /// 「这一台的目录」 needs, since a DSH_HOME may hold a space or a single
    /// quote and the command has to survive the user's shell with it.
    #[test]
    fn a_command_quoting_a_path_survives_both_shells() {
        let path = "/home/demo/o'brien dir";
        let command = ssh_command(TARGET, &format!("printf %s {}", quote(path)));
        assert!(command.starts_with("ssh zjq_aliyun "), "{command}");

        // What the user's shell hands ssh: three words, whatever the path holds.
        let handed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s@@@' {command}"))
            .output()
            .unwrap();
        let handed_text = String::from_utf8_lossy(&handed.stdout).to_string();
        let words: Vec<&str> = handed_text.split("@@@").collect();
        assert_eq!(
            words.len(),
            4,
            "ssh 只该拿到一个参数：{words:?}（路径把它拆开了）"
        );
        assert_eq!(words[0], "ssh");
        assert_eq!(words[1], TARGET);

        // What ssh hands the login shell, and what that shell then runs.
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(words[2])
            .output()
            .expect("本机要有 /bin/sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            path,
            "远端最后执行的是：{}",
            words[2]
        );
    }

    /// The recipes extend the manager the machine already has, and the one for a
    /// machine with none leaves it with both nvm and a Node.
    #[test]
    fn the_install_recipes_extend_what_the_machine_already_has() {
        assert_eq!(node_install_recipe(&["nvm".to_string()]), "nvm install --lts");
        assert_eq!(node_install_recipe(&["fnm".to_string()]), "fnm install --lts");
        assert_eq!(node_install_recipe(&["brew".to_string()]), "brew install node");
        let bare = node_install_recipe(&[]);
        assert!(
            bare.contains("install.sh") && bare.contains("nvm.sh") && bare.contains("nvm install --lts"),
            "没有管理器时要先装 nvm 再在同一个 shell 里加载它（新的 ~/.bashrc 只在下一次生效）：{bare}"
        );
        assert!(pnpm_install_recipe().contains("get.pnpm.io"));

        // The recipes travel inside double quotes, so a double quote in one would
        // end the command early.
        for recipe in [
            node_install_recipe(&[]),
            node_install_recipe(&["nvm".to_string()]),
            node_install_recipe(&["brew".to_string()]),
            pnpm_install_recipe(),
        ] {
            assert!(!recipe.contains('"'), "配方里不能有双引号：{recipe}");
        }
    }


    // -----------------------------------------------------------------------
    // Following and stopping a detached command
    // -----------------------------------------------------------------------

    /// Reads the `alive` answer one real shell produced.
    fn shell_alive(pid: i32) -> bool {
        let out = local_shell(&alive_script(pid));
        parse_marked::<serde_json::Value>(&out).unwrap()["alive"]
            .as_bool()
            .unwrap()
    }

    /// Whether one pid still exists, asked of a real shell.
    fn pid_exists(pid: i32) -> bool {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "if kill -0 {pid} 2>/dev/null; then echo yes; else echo no; fi"
            ))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains("yes")
    }

    /// Waits up to a couple of seconds for a pid to be gone. A process that has
    /// been signalled may take a moment to go away.
    fn wait_gone(pid: i32) -> bool {
        for _ in 0..20 {
            if !pid_exists(pid) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

    /// The rule the whole cancellation story rests on: a detached spawn leads
    /// its own process group, so stopping it means signalling the **group** —
    /// the leader alone leaves the children pnpm started behind, holding the
    /// store's lock. A real group with real children is stopped here, the way
    /// `setsid` gives the launcher's detached spawn one.
    #[test]
    fn stopping_a_detached_command_reaches_the_children_it_started() {
        use std::os::unix::process::CommandExt;

        let dir = scratch("dsh-group");
        let recorded = dir.join("children");
        let fixture = format!(
            "sleep 30 & echo $! >> {file}; sleep 30 & echo $! >> {file}; wait",
            file = recorded.display()
        );
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", &fixture]);
        // Its own process group: pgid == pid, which is what `setsid` gives the
        // launcher's detached spawn and what makes a negative pid the whole tree.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("要能起一个自己的进程组的进程");
        let pid = child.id() as i32;
        assert!(shell_alive(pid), "起好的进程组要在");

        let children = || -> Vec<i32> {
            std::fs::read_to_string(&recorded)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        };
        for _ in 0..50 {
            if children().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let in_group = children();
        assert_eq!(in_group.len(), 2, "夹具要记下组里两个子进程的 pid");
        assert!(in_group.iter().all(|p| pid_exists(*p)));

        let out = local_shell(&stop_group_script(pid));
        decode_ack(&out).expect("停止要成功");

        // The leader is reaped here so the group can be judged; the children
        // are the ones that would have held the store's lock.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = child.wait();
        for child_pid in in_group {
            assert!(
                wait_gone(child_pid),
                "子进程 {child_pid} 没被停掉：那就是会占着 store 锁的孤儿"
            );
        }
        assert!(wait_gone(pid), "组长也要没了");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Stopping something that is already gone is not a failure: the second
    /// cancel click, and an install that exited between the click and the call,
    /// both land here.
    #[test]
    fn stopping_something_already_gone_is_not_an_error() {
        let out = local_shell(&stop_group_script(999_999));
        decode_ack(&out).expect("本来就不在了也算停好了");
    }

    /// The plain pid stop is unchanged: it signals the process it was given and
    /// nothing else, which is what an adopted process needs (ADR-0007).
    #[test]
    fn the_single_pid_stop_still_signals_only_that_pid() {
        let script = stop_pid_script(4242);
        assert!(script.contains("kill -TERM 4242"), "{script}");
        assert!(
            !script.contains("-4242"),
            "不该用进程组的形式（那会连带停掉别人的孩子）：{script}"
        );
    }

    /// A detached command carries its own outcome: the wrapper appends its exit
    /// status to the log the launcher is already reading, because nothing can
    /// `wait` for a process that is not our child.
    #[test]
    fn a_detached_command_writes_its_exit_status_into_its_own_log() {
        let dir = scratch("dsh-measured");
        let target = dir.join("tree");

        let argv = measured_argv(
            &["/usr/bin/env".to_string(), "CI=true".to_string(), "/bin/sh".to_string(), "-c".to_string(), "echo out; echo err >&2; exit 3".to_string()],
            &target.to_string_lossy(),
        );
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&argv[2])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let logged = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        assert!(target.is_dir(), "工作目录要在那里被建出来：{}", target.display());
        assert!(logged.contains("out") && logged.contains("err"), "{logged}");
        let code = logged
            .lines()
            .find_map(exit_status_of)
            .expect("日志里要有退出码那一行");
        assert_eq!(code, 3, "{logged}");
        assert!(
            exit_status_of("added 213 packages").is_none(),
            "其它行不是状态行：{logged}"
        );
        assert!(exit_status_of("").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// One read of a remote log asks only for what was written since the last
    /// one, and it waits **there** until there is something to report: a quiet
    /// install costs one call per wait budget instead of one per second.
    #[test]
    fn a_tail_read_returns_only_what_was_appended_since_the_last_one() {
        let dir = scratch("dsh-tail");
        let log = dir.join("install.log");
        std::fs::write(&log, "line one\n").unwrap();
        let path = log.to_string_lossy().to_string();

        // Caught up and with nothing to watch: it waits the budget out on that
        // side rather than answering at once.
        let started = std::time::Instant::now();
        let out = local_shell(&tail_script(&path, 9, 0, 2));
        let parsed: serde_json::Value = parse_marked(&out).unwrap();
        assert_eq!(parsed["offset"], 9);
        assert_eq!(parsed["data"], "");
        assert!(parsed["alive"].as_bool().unwrap());
        assert!(
            started.elapsed().as_secs() >= 1,
            "等待发生在远端，不是立刻返回后由启动器一遍遍来问"
        );

        // Everything from an offset on, decided there.
        let whole = local_shell(&tail_script(&path, 0, 0, 0));
        let parsed: serde_json::Value = parse_marked(&whole).unwrap();
        assert_eq!(parsed["offset"], 9);
        let first = base64::engine::general_purpose::STANDARD
            .decode(parsed["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&first), "line one\n");

        std::fs::write(&log, "line one\nline two\n").unwrap();
        let rest = local_shell(&tail_script(&path, 9, 0, 0));
        let parsed: serde_json::Value = parse_marked(&rest).unwrap();
        assert_eq!(parsed["offset"], 18);
        let second = base64::engine::general_purpose::STANDARD
            .decode(parsed["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&second), "line two\n");

        // A process that is gone ends the wait at once, however long the budget.
        let quick = std::time::Instant::now();
        let out = local_shell(&tail_script(&path, 18, 999_999, 30));
        let parsed: serde_json::Value = parse_marked(&out).unwrap();
        assert!(!parsed["alive"].as_bool().unwrap());
        assert!(quick.elapsed().as_secs() < 5, "进程没了就不该再等");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Removing a directory on that machine goes by path, and only there: it is
    /// how a cleanup touches the remote data directory and nothing else. A
    /// directory that is not there is already what the caller wanted.
    #[test]
    fn a_remote_cleanup_removes_the_directory_it_is_given() {
        let dir = scratch("dsh-dirs");
        let nested = dir.join("homes").join("0.1.0-rc.6");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("cordis.patch.yml"), "- id: webserver\n").unwrap();
        let sibling = dir.join("homes").join("0.1.0-rc.5");
        std::fs::create_dir_all(&sibling).unwrap();

        let removed = local_shell(&remove_dir_script(&nested.to_string_lossy()));
        decode_ack(&removed).expect("要能删掉目录");
        assert!(!nested.exists(), "目标目录要整棵删掉");
        assert!(sibling.is_dir(), "旁边的目录不该被碰到");
        assert!(dir.is_dir(), "父目录还在");

        // Already gone is already what the caller wanted.
        let again = local_shell(&remove_dir_script(&nested.to_string_lossy()));
        decode_ack(&again).expect("本来就不在了也算删好了");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Which npm decides whether a version is installable *there* is that
    /// machine's own, against the registry its record names.
    #[test]
    fn the_version_lookup_asks_that_machines_own_npm() {
        let with_registry = npm_has_version_script("@deepseek-ai/dsh@1.2.3", Some("https://mirror.example/it's"));
        assert!(with_registry.contains("command -v npm"), "{with_registry}");
        assert!(with_registry.contains("'@deepseek-ai/dsh@1.2.3'"), "{with_registry}");
        assert!(
            with_registry.contains("--registry 'https://mirror.example/it'\\''s'"),
            "主机自己的 registry 要带进去：{with_registry}"
        );

        let plain = npm_has_version_script("@deepseek-ai/dsh@1.2.3", None);
        assert!(
            !plain.contains("--registry"),
            "主机没配 registry 时用那台机器自己的 npm 配置：{plain}"
        );
    }

    /// The transport carries the new capability, and it stops the **group**: the
    /// pid a detached spawn leads, not that process alone. What may not happen
    /// is a signal to the bare pid — that leaves whatever the instance started
    /// running on that machine.
    #[test]
    fn the_process_group_stop_reaches_the_machine_with_that_pid() {
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let transport: &dyn HostTransport = &*fake;

        block_on(transport.stop_group(&host(), 4242)).unwrap();

        assert!(
            fake.calls().contains(&FakeCall::StopGroup(4242)),
            "要停的是那个进程组：{:?}",
            fake.calls()
        );
        assert!(
            !fake.calls().contains(&FakeCall::StopPid(4242)),
            "只发组长就等于把那台机器上实例起的子进程留下了：{:?}",
            fake.calls()
        );
        assert!(stop_group_script(4242).contains("-$dsh_pid"));
    }


    /// Runs one script the way the transport runs it, with a chosen `HOME` —
    /// the remote machine's home directory is not this one, and `~` is expanded
    /// against whichever home the machine's own shell reports.
    fn local_shell_as(script: &str, home: &Path) -> RemoteOutput {
        let out = std::process::Command::new("/bin/bash")
            .arg("-ilc")
            .arg(script)
            .env("HOME", home)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        RemoteOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The launcher follows **this run's** output only. A log that a previous run
    /// wrote to is still there when the next one starts (nothing truncates it),
    /// and its URL line carries a token that belongs to a process which is gone;
    /// reading the size first is what keeps the follower from adopting it.
    #[test]
    fn the_log_size_script_counts_an_absent_file_as_zero_and_a_real_one_as_its_bytes() {
        let dir = scratch("dsh-remote-log-size");
        let log = dir.join("instance.log");
        let path = log.to_string_lossy().to_string();

        #[derive(serde::Deserialize)]
        struct Size {
            offset: u64,
        }
        let absent: Size = parse_marked(&local_shell(&log_size_script(&path))).unwrap();
        assert_eq!(absent.offset, 0, "还没有这个日志：从头开始");

        std::fs::write(&log, "first run\n").unwrap();
        let written: Size = parse_marked(&local_shell(&log_size_script(&path))).unwrap();
        assert_eq!(written.offset, "first run\n".len() as u64);

        let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(r#"{"offset":12}"#)));
        assert_eq!(block_on(log_size(&*fake, &host(), &path)).unwrap(), 12);
        assert!(fake.last_script().contains(&path), "问的是那台机器上的这个日志");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// One profile read, one answer: the manifest's own bytes and a version for
    /// every package the profile has installed — a scoped package under its own
    /// directory name, and a package whose manifest states no version left out
    /// rather than reported as empty. A profile that is not there is one empty
    /// answer, not a failure.
    #[test]
    fn the_profile_read_returns_the_manifest_and_the_installed_versions() {
        let dir = scratch("dsh-remote-profile");
        let web = dir.join("profiles").join("web");
        std::fs::create_dir_all(web.join("node_modules").join("@scope").join("one")).unwrap();
        std::fs::create_dir_all(web.join("node_modules").join("plain")).unwrap();
        std::fs::create_dir_all(web.join("node_modules").join("versionless")).unwrap();
        let manifest = "{\n  \"dsh\": { \"profile\": { \"bundles\": [\"plain\"] } },\n  \"dependencies\": { \"plain\": \"^1\" }\n}\n";
        std::fs::write(web.join("package.json"), manifest).unwrap();
        std::fs::write(
            web.join("node_modules").join("@scope").join("one").join("package.json"),
            "{\n  \"name\": \"@scope/one\",\n  \"version\": \"3.1.4\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            web.join("node_modules").join("plain").join("package.json"),
            r#"{"name":"plain","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            web.join("node_modules").join("versionless").join("package.json"),
            r#"{"name":"versionless"}"#,
        )
        .unwrap();

        let out = local_shell(&profile_packages_script(&web.to_string_lossy()));
        let files = decode_profile_packages(&out).expect("一次调用读回整个 Profile");

        assert_eq!(files.manifest.as_deref(), Some(manifest.as_bytes()));
        assert_eq!(files.installed.get("@scope/one").map(String::as_str), Some("3.1.4"));
        assert_eq!(files.installed.get("plain").map(String::as_str), Some("1.0.0"));
        assert!(
            !files.installed.contains_key("versionless"),
            "清单里没有版本就不报版本：{:?}",
            files.installed
        );

        // 没有这个 Profile：读回一个空的答案，而不是一次失败。
        let absent = local_shell(&profile_packages_script(&dir.join("nope").to_string_lossy()));
        let files = decode_profile_packages(&absent).unwrap();
        assert!(files.manifest.is_none());
        assert!(files.installed.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The local rule for a version's `bin.js` is 「在，而且不是空文件」; a remote
    /// version gets the same question asked of the machine that has the file.
    #[test]
    fn the_file_ready_script_asks_that_machines_shell_whether_the_file_is_there_and_not_empty() {
        let dir = scratch("dsh-remote-file-ready");
        let bin = dir.join("versions 0.1.0/bin.js");
        let path = bin.to_string_lossy().to_string();

        #[derive(serde::Deserialize)]
        struct Ready {
            ready: bool,
        }
        let missing: Ready = parse_marked(&local_shell(&file_ready_script(&path))).unwrap();
        assert!(!missing.ready, "文件不在：没装好");

        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "").unwrap();
        let empty: Ready = parse_marked(&local_shell(&file_ready_script(&path))).unwrap();
        assert!(!empty.ready, "空文件：装到一半");

        std::fs::write(&bin, "#!/usr/bin/env node\n").unwrap();
        let there: Ready = parse_marked(&local_shell(&file_ready_script(&path))).unwrap();
        assert!(there.ready);

        let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(r#"{"ready":true}"#)));
        assert!(block_on(file_ready(&*fake, &host(), &path)).unwrap());
        assert!(fake.last_script().contains(&path), "问的是那台机器上的这个文件");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pid number alone is not an identity: the answer has to be the process's
    /// own start time, and a pid nothing is using has to say so instead of
    /// inventing one. (A running pid's start time is only readable where `/proc`
    /// exists, which is the machine this is aimed at — not the Mac the suite runs
    /// on.)
    #[test]
    fn the_start_time_script_separates_a_live_pid_from_one_nothing_owns() {
        #[derive(serde::Deserialize)]
        struct Started {
            started_at: Option<u64>,
        }
        let gone: Started = parse_marked(&local_shell(&start_time_script(4_000_000))).unwrap();
        assert_eq!(gone.started_at, None, "没人占用的 pid 没有启动时间");

        let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(
            r#"{"started_at":117384720}"#,
        )));
        let started = block_on(process_start_time(&*fake, &host(), 4242)).unwrap();
        assert_eq!(started, Some(117384720));
        assert!(fake.last_script().contains("dsh_pid=4242"), "读的是那个 pid 自己的记录");
    }

    /// `~` is expanded **by the machine that owns the home directory**, and the
    /// answer is the settled absolute path — the user typed a path for a server
    /// they are not sitting at.
    #[test]
    fn the_home_script_expands_a_tilde_against_that_machines_home() {
        let home = scratch("dsh-remote-home");
        let out = local_shell_as(&home_path_script("~/dsh-homes/lab"), &home);
        let resolved: HomePath = parse_marked(&out).expect("真实 shell 的答案要能解析");

        assert_eq!(resolved.path, home.join("dsh-homes/lab").to_string_lossy());
        assert!(
            home.join("dsh-homes/lab").is_dir(),
            "远端校验就是在那台机器上真的把目录建出来"
        );

        std::fs::remove_dir_all(&home).ok();
    }

    /// The settled spelling is what makes one directory one DSH_HOME: a path
    /// typed with `..`, a doubled slash or a trailing slash is the same
    /// directory, so it must come back as the same record path.
    #[test]
    fn the_home_script_settles_the_path_by_text_without_following_links() {
        let home = scratch("dsh-remote-settle");
        let raw = format!("{0}/a//../a/b/", home.to_string_lossy());

        let out = local_shell_as(&home_path_script(&raw), &home);
        let resolved: HomePath = parse_marked(&out).expect("真实 shell 的答案要能解析");

        assert_eq!(resolved.path, home.join("a/b").to_string_lossy());
        assert_eq!(
            decode_names(&local_shell(&list_dir_script(&home.to_string_lossy()))).unwrap().len(),
            1,
            "只留下一层 a"
        );

        std::fs::remove_dir_all(&home).ok();
    }

    /// A path with a space and a single quote in it is one word all the way
    /// through both shells; a path that is not absolute is refused **there**,
    /// with what that machine said.
    #[test]
    fn the_home_script_keeps_awkward_paths_and_refuses_a_relative_one() {
        let home = scratch("dsh-remote-quote");
        let awkward = home.join("we ird/it's here");

        let out = local_shell_as(&home_path_script(&awkward.to_string_lossy()), &home);
        let resolved: HomePath = parse_marked(&out).expect("真实 shell 的答案要能解析");
        assert_eq!(resolved.path, awkward.to_string_lossy());
        assert!(awkward.is_dir());

        let out = local_shell_as(&home_path_script("relative/homes"), &home);
        assert_ne!(out.status, 0, "相对路径不该被当成服务器上的目录");
        assert!(
            out.stderr.contains("绝对路径"),
            "拒绝的理由来自远端：{}",
            out.stderr
        );

        // `~user/...` is not expanded by the machine's shell and must not
        // silently become a directory here.
        let out = local_shell_as(&home_path_script("~someone/.dsh"), &home);
        assert_ne!(out.status, 0);
        assert!(out.stderr.contains("~用户名"), "{}", out.stderr);

        std::fs::remove_dir_all(&home).ok();
    }

    /// The profile list is the same list the local reader yields: directories
    /// only, and the two reserved names left out. A HOME with no `profiles/`
    /// yet is an empty list rather than a failure.
    #[test]
    fn the_profiles_script_lists_directories_and_skips_the_reserved_names() {
        let home = scratch("dsh-remote-profiles");
        let profiles = home.join("profiles");
        std::fs::create_dir_all(profiles.join("web")).unwrap();
        std::fs::create_dir_all(profiles.join("demo")).unwrap();
        std::fs::create_dir_all(profiles.join("__temp__")).unwrap();
        std::fs::create_dir_all(profiles.join("node_modules")).unwrap();
        std::fs::write(profiles.join("some-file"), "not a profile").unwrap();

        let out = local_shell(&profiles_script(&profiles.to_string_lossy()));
        assert_eq!(
            decode_profiles(&out).unwrap(),
            vec!["demo".to_string(), "web".to_string()]
        );

        let empty = home.join("nothing-here");
        let out = local_shell(&profiles_script(
            &empty.join("profiles").to_string_lossy(),
        ));
        assert!(decode_profiles(&out).unwrap().is_empty());

        std::fs::remove_dir_all(&home).ok();
    }

    /// Copy, rename and remove each do one thing on that machine, and each says
    /// what happened — including the two refusals the local flow makes
    /// (「不存在」 and 「已存在」) rather than overwriting anything.
    #[test]
    fn the_profile_scripts_copy_rename_and_remove_on_that_machine() {
        let home = scratch("dsh-remote-profile-ops");
        let profiles = home.join("profiles");
        std::fs::create_dir_all(profiles.join("web/app")).unwrap();
        std::fs::write(profiles.join("web/package.json"), "{}").unwrap();
        let from = profiles.join("web").to_string_lossy().to_string();
        let to = profiles.join("web copy").to_string_lossy().to_string();

        decode_ack(&local_shell(&copy_profile_script(&from, &to))).unwrap();
        assert!(profiles.join("web copy/app").is_dir(), "整棵树都复制过去");
        assert!(profiles.join("web copy/package.json").is_file());

        // A target that is already there, and a source that is not: neither may
        // silently overwrite or half-create anything.
        let out = local_shell(&copy_profile_script(&from, &to));
        assert_ne!(out.status, 0);
        assert!(out.stderr.contains("远端已经有这个目录"), "{}", out.stderr);
        let missing = profiles.join("nope").to_string_lossy().to_string();
        let out = local_shell(&copy_profile_script(&missing, &to));
        assert_ne!(out.status, 0);
        assert!(out.stderr.contains("远端没有这个 Profile 目录"), "{}", out.stderr);

        let renamed = profiles.join("renamed").to_string_lossy().to_string();
        decode_ack(&local_shell(&rename_profile_script(&to, &renamed))).unwrap();
        assert!(profiles.join("renamed").is_dir());
        assert!(!profiles.join("web copy").exists());

        decode_ack(&local_shell(&delete_profile_script(&renamed))).unwrap();
        assert!(!profiles.join("renamed").exists());
        assert!(profiles.join("web").is_dir(), "删一个不动另一个");

        std::fs::remove_dir_all(&home).ok();
    }

    // -----------------------------------------------------------------------
    // The terminal session on the other machine
    // -----------------------------------------------------------------------

    /// A terminal on this Mac that lands in the instance's DSH_HOME **there**:
    /// one command line, two shells. The path may hold a space and a single
    /// quote, so the script is quoted once for the shell in the user's terminal
    /// and once for the login shell at the other end — and running it for real
    /// is what proves both.
    #[test]
    fn the_terminal_command_lands_in_the_homes_directory_through_both_shells() {
        // A directory that really carries both characters: the round trip is
        // observed, not trusted.
        let dir = std::env::temp_dir().join(format!("dsh t11'q-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let home = dir.to_string_lossy().to_string();

        let env = vec![
            ("DSH_HOME".to_string(), home.clone()),
            (
                "DSH_LAUNCHER_INSTANCE".to_string(),
                "服务器 实例".to_string(),
            ),
        ];
        let path_dirs = vec!["/home/demo/.local/share/dsh-launcher/versions/0.1.0/node_modules/.bin"
            .to_string()];
        let script = terminal_script(&home, &env, &path_dirs);
        let command = terminal_command(TARGET, &script);
        assert!(command.starts_with("ssh -t zjq_aliyun "), "{command}");

        // What the user's own shell hands ssh. The separator is not a newline:
        // the script itself has them, and argv is what is being read here.
        assert!(!script.contains("@@@"), "分隔符不能出现在脚本里：{script}");
        let handed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s@@@' {command}"))
            .output()
            .unwrap();
        let handed_text = String::from_utf8_lossy(&handed.stdout).to_string();
        let mut words: Vec<String> = handed_text.split("@@@").map(str::to_string).collect();
        assert_eq!(words.pop().as_deref(), Some(""), "最后一个 @@ 后面不该有东西");
        assert_eq!(words[0], "ssh");
        assert_eq!(words[1], "-t", "远端会话要有终端，否则 shell 不是交互的");
        assert_eq!(words[2], TARGET);
        assert_eq!(words.len(), 4, "ssh 只该拿到一个参数（脚本可能被换行拆开了）");

        // What ssh hands the remote login shell: its arguments joined with
        // spaces. Running that here is what the machine does with it — the
        // session ends up in the DSH_HOME with the launcher's variables set.
        // The exec'd login shell reads the piped lines, which is how the state
        // in front of the exec is read back.
        let remote = words[3..].join(" ");
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&remote)
            .env("SHELL", "/bin/sh")
            .env("TERM", "dumb")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            let stdin = child.stdin.as_mut().unwrap();
            stdin
                .write_all(b"printf '>>>%s\\n' \"$PWD\" \"$DSH_HOME\" \"$DSH_LAUNCHER_INSTANCE\"\n")
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        let printed: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .split(">>>")
            .skip(1)
            .map(|chunk| chunk.lines().next().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            printed,
            vec![
                home.clone(),
                home.clone(),
                "服务器 实例".to_string()
            ],
            "远端会话要 cd 进那个目录，并带上这两个变量：{:?} / {:?} / {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            out.status
        );
        assert!(
            script.contains(&format!(
                "export PATH={}:",
                quote(path_dirs[0].as_str())
            )),
            "版本的 bin 目录要排在最前（远端不写 wrapper，pnpm 自己的 .bin 就是入口）：{script}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // -----------------------------------------------------------------------
    // The last stretch of a remote log
    // -----------------------------------------------------------------------

    /// Opening a remote log copies home only the end of it: a DSH log grows
    /// without limit, and one that has been running for weeks is not something
    /// to hand back whole.
    #[test]
    fn the_log_tail_script_reads_the_last_bytes_of_one_file() {
        let dir = scratch("dsh-remote-log-tail");
        let log = dir.join("i1.log");
        let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&log, &text).unwrap();

        let out = local_shell(&log_tail_script(&log.to_string_lossy(), 40));
        let tail = decode_log_tail(&out).unwrap();
        assert!(tail.exists);
        assert_eq!(tail.text, text[text.len() - 40..], "只读到末尾那一段");
        assert!(tail.text.len() <= 40);

        // A log that is not there yet is an answer, not a failure: an instance
        // that has never printed anything has an empty log.
        let out = local_shell(&log_tail_script(
            &dir.join("nothing.log").to_string_lossy(),
            40,
        ));
        let missing = decode_log_tail(&out).unwrap();
        assert!(!missing.exists);
        assert!(missing.text.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The same read through the seam: one call, and the one thing a test can
    /// hold the launcher to — it asked for the log on the right machine and
    /// asked for no more than the bound.
    #[test]
    fn the_log_tail_asks_that_machine_for_a_bounded_stretch_of_the_log() {
        let fake = FakeTransport::new().answering(FakeAnswer::stdout(marked(
            &serde_json::json!({"exists": true, "data": base64::engine::general_purpose::STANDARD.encode("hello\n")}).to_string(),
        )));
        let transport: &dyn HostTransport = &*fake;

        let tail = block_on(log_tail(
            transport,
            &host(),
            "/home/demo/.local/share/dsh-launcher/logs/i1.log",
            LOG_TAIL_BYTES,
        ))
        .unwrap();

        assert_eq!(tail.text, "hello\n");
        assert!(tail.exists);
        let script = fake.last_script();
        assert!(
            script.contains(&quote("/home/demo/.local/share/dsh-launcher/logs/i1.log")),
            "{script}"
        );
        assert!(
            script.contains(&format!("dsh_bytes={LOG_TAIL_BYTES}"))
                && script.contains("tail -c \"$dsh_bytes\""),
            "读的不是有界的一段：{script}"
        );
    }

    /// One question, one answer: whether that machine already has a `~/.dsh`.
    #[test]
    fn the_user_home_script_answers_that_machines_home_and_its_dsh_folder() {
        let home = scratch("dsh-remote-user-home");

        let out = local_shell_as(&user_home_script(), &home);
        let before: RemoteUserHome = parse_marked(&out).unwrap();
        assert_eq!(before.home, home.to_string_lossy());
        assert!(!before.exists, "还没有 ~/.dsh");
        assert!(before.dsh_home.is_empty());

        std::fs::create_dir_all(home.join(".dsh")).unwrap();
        let out = local_shell_as(&user_home_script(), &home);
        let after: RemoteUserHome = parse_marked(&out).unwrap();
        assert!(after.exists);
        assert_eq!(after.dsh_home, home.join(".dsh").to_string_lossy());

        std::fs::remove_dir_all(&home).ok();
    }

    /// The one thing no fake can show: that a real machine's own shell reads a
    /// real profile — GNU `base64`, `grep -o`, `cut`, the `@scope/pkg` walk and
    /// the marker envelope — and answers in one call with what the page
    /// projects from.
    ///
    /// ```text
    /// DSH_LAUNCHER_TEST_SSH_HOST=zjq_aliyun \
    ///   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored a_real_profile
    /// ```
    ///
    /// It writes one throwaway profile directory under that machine's `/tmp` and
    /// removes it again.
    #[tokio::test]
    #[ignore = "需要一台能免密登录的 ssh 主机（DSH_LAUNCHER_TEST_SSH_HOST）"]
    async fn a_real_profile_read_answers_from_that_machine() {
        let Ok(target) = std::env::var("DSH_LAUNCHER_TEST_SSH_HOST") else {
            eprintln!("跳过：没有设置 DSH_LAUNCHER_TEST_SSH_HOST");
            return;
        };
        let socket_dir = PathBuf::from("/tmp").join(format!(
            "d10{}",
            uuid::Uuid::new_v4().simple().to_string().split_at(6).0
        ));
        let host = RemoteHost::of(
            &crate::config::Host {
                id: "host-real".into(),
                display_name: "真机".into(),
                ssh_target: target,
                registry: None,
                extra_env: Default::default(),
                data_dir: None,
                builtin: false,
                toolchain: Default::default(),
                toolchain_overrides: Default::default(),
            },
            &socket_dir,
        )
        .unwrap();
        let transport = crate::ssh::SshTransport::new();
        let transport: &dyn HostTransport = &transport;

        let dir = format!(
            "/tmp/dsh-profile-real-{}",
            uuid::Uuid::new_v4().simple()
        );
        let web = format!("{dir}/profiles/web");
        let manifest = r#"{"dsh":{"profile":{"bundles":["plain"]}},"dependencies":{"plain":"^1"}}"#;
        transport
            .write_file(&host, &format!("{web}/package.json"), manifest.as_bytes().to_vec())
            .await
            .unwrap();
        transport
            .write_file(
                &host,
                &format!("{web}/node_modules/plain/package.json"),
                br#"{"name":"plain","version":"1.2.3"}"#.to_vec(),
            )
            .await
            .unwrap();
        transport
            .write_file(
                &host,
                &format!("{web}/node_modules/@scope/one/package.json"),
                br#"{"name":"@scope/one","version":"4.5.6"}"#.to_vec(),
            )
            .await
            .unwrap();

        let files = profile_packages(transport, &host, &web)
            .await
            .expect("真机上的一次调用要读回整个 Profile");

        assert_eq!(files.manifest.as_deref(), Some(manifest.as_bytes()));
        assert_eq!(files.installed.get("plain").map(String::as_str), Some("1.2.3"));
        assert_eq!(
            files.installed.get("@scope/one").map(String::as_str),
            Some("4.5.6")
        );

        remove_dir(transport, &host, &dir).await.unwrap();
        assert!(
            read_file_optional(transport, &host, &format!("{web}/package.json"))
                .await
                .unwrap()
                .is_none(),
            "临时目录要删干净"
        );
        std::fs::remove_dir_all(&socket_dir).ok();
    }
}
