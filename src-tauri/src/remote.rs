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

/// One command the user runs **on this Mac** to install a tool on a remote host.
///
/// Quoted for two shells because it goes through two: the user's own shell has
/// to hand ssh a single argument, and ssh joins its arguments with spaces and
/// lets the remote shell split them again. The inner quotes are what make the
/// recipe reach `bash -ilc` as one word — the same shape
/// [`script_command`] uses, so the install runs in the shell the probe read.
///
/// A recipe therefore must not contain a double quote; the ones below are
/// written without one.
pub fn ssh_command(target: &str, recipe: &str) -> String {
    let shell = crate::ssh::REMOTE_SHELL;
    format!("ssh {target} '{shell} -ilc \"{recipe}\"'")
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

/// Reads one file, as base64 — the ssh channel is text, and version manifests,
/// icons and patch documents must arrive byte for byte.
const READ_FILE: &str = r#"set -u
dsh_path=%%PATH%%
if [ ! -f "$dsh_path" ]; then
  printf '远端没有这个文件：%s\n' "$dsh_path" >&2
  exit 2
fi
dsh_data=$(base64 "$dsh_path" | tr -d '\n')
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
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) enum FakeCall {
        Run(String),
        Spawn(String),
        StopPid(i32),
        ReadFile(String),
        WriteFile(String, Vec<u8>),
        ListDir(String),
        OpenForward(PortForward),
        CloseForward(PortForward),
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

    /// One tool of an environment probe's answer, as the script shapes it.
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
            _host: &'a RemoteHost,
            forward: PortForward,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(FakeCall::OpenForward(forward));
                match self.answer()? {
                    FakeAnswer::Done => Ok(()),
                    other => Err(wrong_shape(&other)),
                }
            })
        }

        fn close_forward<'a>(
            &'a self,
            _host: &'a RemoteHost,
            forward: PortForward,
        ) -> HostFuture<'a, Result<(), HostError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(FakeCall::CloseForward(forward));
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
}
