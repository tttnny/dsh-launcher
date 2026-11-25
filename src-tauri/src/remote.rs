use crate::ssh::{
    HostError, HostErrorKind, HostTransport, RemoteCall, RemoteHost, RemoteOutput, Shell,
    DEFAULT_TIMEOUT,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const MARK: &str = "<<<DSH-JSON>>>";
pub const END: &str = "<<<DSH-END>>>";

pub const EXIT_MARK: &str = "<<<DSH-EXIT>>>";

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn parse_marked<T: DeserializeOwned>(out: &RemoteOutput) -> Result<T, HostError> {
    let payload =
        between_markers(&out.stdout).ok_or_else(|| failed(out, "远端脚本没有返回结果标记"))?;
    serde_json::from_str(payload).map_err(|e| failed(out, &format!("远端脚本的结果无法解析：{e}")))
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

pub async fn json<T: DeserializeOwned>(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    script: String,
    timeout: Duration,
    shell: Shell,
) -> Result<T, HostError> {
    let call = RemoteCall::within(script, timeout);
    let call = match shell {
        Shell::Plain => call,
        Shell::Login => call.with_login_shell(),
    };
    let out = transport.run(host, call).await?;
    parse_marked(&out)
}

const NPM_VIEW_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSystem {
    pub os: String,
    pub release: String,
    pub arch: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTool {
    pub path: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnectionTest {
    pub ok: bool,
    pub kind: Option<HostErrorKind>,
    pub message: Option<String>,
    pub system: Option<RemoteSystem>,
    pub home: Option<String>,
    pub data_dir: Option<String>,
    pub data_dir_problem: Option<String>,
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
            data_dir_problem: None,
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
            data_dir: facts.data_dir,
            data_dir_problem: facts.data_dir_problem,
            data_dir_exists: facts.data_dir_exists,
            node: facts.node,
            pnpm: facts.pnpm,
        }
    }
}

#[derive(Deserialize)]
struct Facts {
    os: String,
    release: String,
    arch: String,
    home: String,
    data_dir: Option<String>,
    data_dir_problem: Option<String>,
    data_dir_exists: bool,
    node: Option<RemoteTool>,
    pnpm: Option<RemoteTool>,
}

pub async fn connection_test(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    data_dir: Option<&str>,
) -> ConnectionTest {
    let script = connection_test_script(data_dir);
    match json::<Facts>(transport, host, script, DEFAULT_TIMEOUT, Shell::Login).await {
        Ok(facts) => ConnectionTest::answered(facts),
        Err(err) => ConnectionTest::refused(err),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInstalledVersion {
    pub name: String,
    pub dir: String,
    pub bin_ready: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct RemotePathLink {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteVersions {
    pub versions: Vec<RemoteInstalledVersion>,
    pub link: RemotePathLink,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NpmListings {
    pub versions: String,
    pub time: String,
}

pub async fn installed_versions(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    versions_dir: &str,
    known: &[&str],
) -> Result<RemoteVersions, HostError> {
    let out = transport
        .run(
            host,
            RemoteCall::new(installed_versions_script(versions_dir, known)),
        )
        .await?;
    decode_remote_versions(&out)
}

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
        Shell::Login,
    )
    .await
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailChunk {
    pub text: String,
    pub offset: u64,
    pub alive: bool,
}

pub const MAX_TAIL_BYTES: usize = 256 * 1024;

const WAIT_OVERHEAD_SECS: u64 = 30;

fn budget_for(wait_secs: u64) -> Duration {
    Duration::from_secs(wait_secs + WAIT_OVERHEAD_SECS)
}

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
    let out = transport
        .run(host, RemoteCall::within(script, budget_for(wait_secs)))
        .await?;
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

pub(crate) const MAX_LOG_LINE_BYTES: usize = 64 * 1024;

pub(crate) const CUT_LINE_PREFIX_BYTES: usize = 1024;

pub(crate) fn take_log_lines(pending: &mut String, lines: &mut Vec<String>) -> bool {
    while let Some(at) = pending.find('\n') {
        lines.push(pending[..at].trim_end_matches('\r').to_string());
        pending.drain(..=at);
    }
    if pending.len() <= MAX_LOG_LINE_BYTES {
        return false;
    }
    let mut end = CUT_LINE_PREFIX_BYTES.min(pending.len());
    while !pending.is_char_boundary(end) {
        end -= 1;
    }
    lines.push(pending[..end].to_string());
    pending.clear();
    true
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePortOwner {
    pub pid: Option<i32>,
    pub command: String,
    pub args: String,
}

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
        #[serde(default)]
        args: String,
        #[serde(default)]
        error: Option<String>,
    }
    let out = transport
        .run(host, RemoteCall::new(port_owner_script(port)))
        .await?;
    let answer: Answer = parse_marked(&out)?;
    if let Some(error) = answer.error {
        return Err(HostError::remote_command(format!(
            "在那台机器上探测端口占用失败：{error}"
        )));
    }
    Ok(answer.occupied.then_some(RemotePortOwner {
        pid: answer.pid,
        command: answer.command,
        args: answer.args,
    }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteMint {
    pub status: u16,
    pub cookie: Option<String>,
}

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

const READ_FILE_MISSING: i32 = 2;

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
            RemoteCall::new(npm_has_version_script(package, registry)).with_login_shell(),
        )
        .await?;
    Ok(parse_marked::<Answer>(&out)?.exists)
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RemoteSkipped {
    pub path: String,
    #[serde(default)]
    pub version: String,
    pub reason: String,
    #[serde(default)]
    pub major: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RemoteToolStatus {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub skipped: Vec<RemoteSkipped>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemoteEnvironment {
    pub shell: String,
    #[serde(default)]
    pub managers: Vec<String>,
    pub node: RemoteToolStatus,
    pub pnpm: RemoteToolStatus,
}

pub async fn environment(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    manual_node: Option<&str>,
    manual_pnpm: Option<&str>,
) -> Result<RemoteEnvironment, HostError> {
    let script = environment_script(manual_node, manual_pnpm);
    json::<RemoteEnvironment>(transport, host, script, ENVIRONMENT_TIMEOUT, Shell::Login).await
}

const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualProbe {
    Version(String),
    Missing,
    Unusable,
}

pub async fn probe_manual_tool(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<ManualProbe, HostError> {
    let facts: ManualFacts = json(
        transport,
        host,
        manual_tool_script(path),
        DEFAULT_TIMEOUT,
        Shell::Plain,
    )
    .await?;
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

pub fn ssh_command(target: &str, recipe: &str) -> String {
    let shell = crate::ssh::REMOTE_SHELL;
    format!(
        "ssh {target} {}",
        quote(&format!("{shell} -ilc \"{recipe}\""))
    )
}

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

pub fn pnpm_install_recipe() -> &'static str {
    "curl -fsSL https://get.pnpm.io/install.sh | sh -"
}

const ESCAPER: &str = r#"dsh_esc() {
  printf '%s' "$1" | tr -d '\000-\037' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}"#;

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

# The data directory is settled **here**, once: `~` is this machine's home
# directory, one directory typed two ways is one path, and a value that cannot be
# settled is answered as a reason instead of guessed at. That machine's answer is
# what the launcher writes down, so every later script is handed the settled
# absolute path and none of them has to know what a `~` meant.
dsh_configured=%%DATA_DIR%%
if [ -z "$dsh_configured" ]; then dsh_configured='~/.local/share/dsh-launcher'; fi
dsh_problem=
case "$dsh_configured" in
  '~') dsh_data="${HOME:-}" ;;
  '~/'*) dsh_data="${HOME:-}/${dsh_configured#\~/}" ;;
  '~'*) dsh_data=; dsh_problem="不支持 ~用户名 形式的路径：$dsh_configured" ;;
  /*) dsh_data="$dsh_configured" ;;
  *) dsh_data=; dsh_problem="请填写绝对路径（以 / 开头），或用 ~ 表示那台机器的家目录：$dsh_configured" ;;
esac
if [ -z "$dsh_problem" ]; then
  # Settle by text: one directory typed two ways is one data directory.
  dsh_rest=${dsh_data#/}
  dsh_data=
  while [ -n "$dsh_rest" ]; do
    dsh_part=${dsh_rest%%/*}
    if [ "$dsh_rest" = "$dsh_part" ]; then dsh_rest=; else dsh_rest=${dsh_rest#*/}; fi
    case "$dsh_part" in
      ''|.) ;;
      ..) dsh_data=${dsh_data%/*} ;;
      *) dsh_data="$dsh_data/$dsh_part" ;;
    esac
  done
  [ -n "$dsh_data" ] || dsh_data=/
fi

if [ -n "$dsh_problem" ]; then
  dsh_dir_json='"data_dir":null,"data_dir_problem":"'"$(dsh_esc "$dsh_problem")"'"'
  dsh_exists=false
else
  dsh_dir_json='"data_dir":"'"$(dsh_esc "$dsh_data")"'","data_dir_problem":null'
  if [ -d "$dsh_data" ]; then dsh_exists=true; else dsh_exists=false; fi
fi

dsh_json=$(printf '{"os":"%s","release":"%s","arch":"%s","home":"%s",%s,"data_dir_exists":%s,"node":%s,"pnpm":%s}' \
  "$(dsh_esc "$(uname -s)")" \
  "$(dsh_esc "$(uname -r)")" \
  "$(dsh_esc "$(uname -m)")" \
  "$(dsh_esc "${HOME:-}")" \
  "$dsh_dir_json" \
  "$dsh_exists" \
  "$(dsh_tool node)" \
  "$(dsh_tool pnpm)")
printf '<<<DSH-JSON>>>%s<<<DSH-END>>>' "$dsh_json"
"#;

const LINK_STATE: &str = r#"dsh_marker=%%MARKER%%
dsh_link="${HOME:-}/.local/bin/dsh"

# A directory this link names is one of the launcher's versions. The loop
# variable is `dsh_known`, not one of the caller's: this block is spliced into
# scripts that keep the directory they are asking about in their own variables,
# and a shared name would let it overwrite one.
dsh_is_version() {
  [ -n "$1" ] || return 1
  dsh_old_ifs=$IFS
  IFS='
'
  for dsh_known in $dsh_versions; do
    [ "$dsh_known" = "$1" ] && { IFS=$dsh_old_ifs; return 0; }
  done
  IFS=$dsh_old_ifs
  return 1
}

dsh_symlink_into() {
  dsh_old_ifs=$IFS
  IFS='
'
  for dsh_known in $dsh_versions; do
    case "$1" in
      "$dsh_known"|"$dsh_known"/*) { IFS=$dsh_old_ifs; return 0; } ;;
    esac
  done
  IFS=$dsh_old_ifs
  return 1
}

dsh_state=absent
dsh_target=
if [ -L "$dsh_link" ]; then
  dsh_state=symlink
  dsh_raw=$(readlink "$dsh_link" 2>/dev/null || true)
  case "$dsh_raw" in
    /*) dsh_target="$dsh_raw" ;;
    *) dsh_target="$(dirname "$dsh_link")/$dsh_raw" ;;
  esac
elif [ -e "$dsh_link" ]; then
  dsh_state=foreign
  if [ -f "$dsh_link" ]; then
    dsh_written=$(head -n 3 "$dsh_link" 2>/dev/null | sed -n "s|^$dsh_marker[[:space:]]*\(.*\)$|\1|p" | head -n 1 | tr -d '\r')
    if [ -n "$dsh_written" ] && dsh_is_version "$dsh_written"; then
      dsh_state=script
      dsh_target="$dsh_written"
    fi
  fi
fi

dsh_ours=false
case "$dsh_state" in
  symlink) dsh_symlink_into "$dsh_target" && dsh_ours=true ;;
  script) dsh_ours=true ;;
esac
"#;

const INSTALLED_VERSIONS: &str = r#"set -u
%%ESCAPER%%
dsh_versions_dir=%%PATH%%
dsh_versions=%%VERSIONS%%
%%LINK%%
dsh_rows=
dsh_sep=
for dsh_entry in "$dsh_versions_dir"/*; do
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

printf '<<<DSH-JSON>>>{"versions":[%s],"link":{"state":"%s","target":"%s"}}<<<DSH-END>>>' \
  "$dsh_rows" "$dsh_state" "$(dsh_esc "$dsh_target")"
"#;

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

fn link_state() -> String {
    fill(
        LINK_STATE,
        &[(
            "%%MARKER%%",
            quote(&format!("{} ", crate::pathlink::MARKER.trim())),
        )],
    )
}

pub fn installed_versions_script(versions_dir: &str, versions: &[&str]) -> String {
    fill(
        INSTALLED_VERSIONS,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATH%%", quote(versions_dir)),
            ("%%VERSIONS%%", quote(&versions.join("\n"))),
            ("%%LINK%%", link_state()),
        ],
    )
}
pub fn available_versions_script(registry: &str) -> String {
    fill(
        AVAILABLE_VERSIONS,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%REGISTRY%%", quote(registry)),
        ],
    )
}

pub fn connection_test_script(data_dir: Option<&str>) -> String {
    fill(
        CONNECTION_TEST,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%DATA_DIR%%", data_dir.map(quote).unwrap_or_default()),
        ],
    )
}

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

pub fn environment_script(manual_node: Option<&str>, manual_pnpm: Option<&str>) -> String {
    environment_script_with(manual_node, manual_pnpm, &fallback_bins())
}

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
            ("%%PNPM_MIN%%", crate::toolchain::MIN_PNPM_MAJOR.to_string()),
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
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATH%%", quote(path)),
        ],
    )
}

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

const CREATE_PROFILE: &str = r#"set -u
%%ESCAPER%%
dsh_profiles=%%PROFILES%%
dsh_name=%%NAME%%
dsh_user_default=%%USER_DEFAULT%%
dsh_homes_root=%%HOMES%%

# A refusal names its reason in the same acknowledgement shape a success uses:
# `decode_ack` then hands the caller `Profile「x」已存在` instead of a script that
# answered nothing. `ok` stays the field every caller reads.
dsh_refuse() {
  printf '<<<DSH-JSON>>>{"ok":false,"error":"%s"}<<<DSH-END>>>' "$(dsh_esc "$1")"
  exit 3
}

if [ ! -d "$dsh_profiles" ]; then
  mkdir -p "$dsh_profiles" || dsh_refuse "创建 profiles 目录失败：$dsh_profiles"
fi
dsh_target="$dsh_profiles/$dsh_name"
if [ -e "$dsh_target" ]; then
  dsh_refuse "Profile「$dsh_name」已存在"
fi

# The webserver port a copied template carries: DSH honors the patch over the
# CLI --port, so a copy that keeps it binds a port another instance may own.
dsh_scrub_port() {
  dsh_patch="$1/cordis.patch.yml"
  [ -f "$dsh_patch" ] || return 0
  dsh_new=$(awk '
    /^- id:/ { in_block = ($0 ~ /^- id: webserver/) ; print ; next }
    in_block && /^[[:space:]]*port:/ {
      match($0, /^[[:space:]]*/) ; indent = substr($0, 1, RLENGTH)
      print indent "port: 0" ; next
    }
    { print }
  ' "$dsh_patch")
  [ -n "$dsh_new" ] && printf '%s\n' "$dsh_new" > "$dsh_patch"
}

dsh_template=
# The user's own `~/.dsh` is not the launcher's to put a template in, on any
# machine: the path says so directly, whichever record named this HOME.
if [ "$dsh_profiles" = "${HOME:-}/.dsh/profiles" ]; then
  dsh_user_default=true
fi
if [ "$dsh_user_default" != true ] && [ -d "$dsh_profiles/__temp__" ]; then
  dsh_template="$dsh_profiles/__temp__"
elif [ "$dsh_user_default" != true ] && [ -d "$dsh_profiles/web" ]; then
  dsh_template="$dsh_profiles/__temp__"
  cp -R "$dsh_profiles/web" "$dsh_template" || dsh_refuse '初始化模板失败'
  dsh_scrub_port "$dsh_template"
else
  for dsh_candidate in "$dsh_homes_root"/*/profiles/__temp__; do
    [ -d "$dsh_candidate" ] || continue
    dsh_template="$dsh_candidate"
    break
  done
  if [ -z "$dsh_template" ]; then
    dsh_refuse '未找到可用的 Profile __temp__ 模板，请先安装至少一个 DSH 版本'
  fi
fi

cp -R "$dsh_template" "$dsh_target" || dsh_refuse '创建 Profile 失败'
printf '<<<DSH-JSON>>>{"ok":true,"name":"%s"}<<<DSH-END>>>' "$(printf '%s' "$dsh_name" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
"#;

const PATH_LINK: &str = r#"set -u
dsh_action=%%ACTION%%
dsh_version=%%VERSION%%
dsh_versions=%%VERSIONS%%

if [ -z "${HOME:-}" ]; then
  printf '读不到那台机器的 HOME，无法定位 ~/.local/bin\n' >&2
  exit 2
fi
%%LINK%%

dsh_remove_link() {
  [ -e "$dsh_link" ] || [ -L "$dsh_link" ] || return 0
  rm -f "$dsh_link" || { printf '删除 %s 失败\n' "$dsh_link" >&2; exit 6; }
}

dsh_finish() {
  printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
}

# Says why a link that is not the launcher's is being left alone. The wording is
# the local one's (`crate::pathlink`), so both machines answer the same way.
dsh_refuse() {
  case "$dsh_state" in
    symlink) printf '~/.local/bin/dsh 指向 %s，不是启动器登记的版本，没有改动它\n' "$dsh_target" >&2 ;;
    *) printf '~/.local/bin/dsh 不是启动器登记的，没有改动它\n' >&2 ;;
  esac
  exit 3
}

case "$dsh_action" in
  unregister)
    if [ "$dsh_state" = absent ]; then dsh_finish; fi
    if [ "$dsh_ours" != true ]; then dsh_refuse; fi
    dsh_remove_link
    dsh_finish
    ;;
  unlink)
    # Deleting a version withdraws its own link only: what it names has to be
    # inside this version's directory. An empty version (a caller that named
    # none) matches nothing — the pattern would otherwise be `"" | "/*"`.
    if [ -n "$dsh_version" ] && [ "$dsh_ours" = true ]; then
      case "$dsh_target" in
        "$dsh_version"|"$dsh_version"/*) dsh_remove_link ;;
      esac
    fi
    dsh_finish
    ;;
  register)
    if [ "$dsh_state" != absent ] && [ "$dsh_ours" != true ]; then
      case "$dsh_state" in
        symlink) printf '~/.local/bin/dsh 已经指向 %s，不是启动器安装的版本；先把它移走再登记\n' "$dsh_target" >&2 ;;
        *) printf '~/.local/bin/dsh 已经存在，而且不是启动器登记的；先把它移走再登记\n' >&2 ;;
      esac
      exit 4
    fi

    dsh_entry="$dsh_version/node_modules/.bin/dsh"
    dsh_bin="$dsh_version/node_modules/@deepseek-ai/dsh/lib/bin.js"
    if [ -f "$dsh_version/apps/cli/package.json" ]; then
      dsh_bin="$dsh_version/apps/cli/lib/bin.js"
    fi
    if [ -f "$dsh_entry" ]; then dsh_target="$dsh_entry"; else dsh_target="$dsh_bin"; fi
    if [ ! -f "$dsh_target" ]; then
      printf '这个版本没有 dsh 启动文件（%s），先把它装好再登记\n' "$dsh_target" >&2
      exit 5
    fi

    mkdir -p "$HOME/.local/bin" || { printf '建立 %s 失败\n' "$HOME/.local/bin" >&2; exit 6; }
    dsh_remove_link

    if [ -x "$dsh_target" ]; then
      ln -s "$dsh_target" "$dsh_link" || { printf '在 %s 建立 dsh 命令失败\n' "$dsh_link" >&2; exit 7; }
    else
      # The entry is a Node script without its own executable bit: the wrapper
      # says so and runs it with that machine's own node.
      {
        printf '#!/bin/sh\n'
        printf '%s%s\n' "$dsh_marker" "$dsh_version"
        printf 'exec node %s "$@"\n' "$(printf "'%s'" "$(printf '%s' "$dsh_target" | sed "s/'/'\\\\''/g")")"
      } > "$dsh_link" || { printf '在 %s 建立 dsh 命令失败\n' "$dsh_link" >&2; exit 7; }
      chmod 755 "$dsh_link" || { printf '设置 %s 的可执行权限失败\n' "$dsh_link" >&2; exit 8; }
    fi
    dsh_finish
    ;;
  *)
    printf '未知的登记操作：%s\n' "$dsh_action" >&2
    exit 9
    ;;
esac
"#;

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

pub fn path_link_script(action: &str, version: Option<&str>, versions: &[&str]) -> String {
    fill(
        PATH_LINK,
        &[
            ("%%ACTION%%", action.to_string()),
            ("%%VERSION%%", version.map(quote).unwrap_or_default()),
            ("%%VERSIONS%%", quote(&versions.join("\n"))),
            ("%%LINK%%", link_state()),
        ],
    )
}

pub async fn path_link(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    action: &str,
    version: Option<&str>,
    versions: &[&str],
) -> Result<(), HostError> {
    let out = transport
        .run(
            host,
            RemoteCall::new(path_link_script(action, version, versions)),
        )
        .await?;
    decode_ack(&out)
}

pub async fn create_profile(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    profiles_dir: &str,
    name: &str,
    homes_root: &str,
    user_default: bool,
) -> Result<String, HostError> {
    let out = transport
        .run(
            host,
            RemoteCall::new(create_profile_script(
                profiles_dir,
                name,
                homes_root,
                user_default,
            )),
        )
        .await?;
    #[derive(Deserialize)]
    struct Created {
        name: String,
    }
    Ok(parse_marked::<Created>(&out)?.name)
}

pub fn create_profile_script(
    profiles_dir: &str,
    name: &str,
    homes_root: &str,
    user_default: bool,
) -> String {
    fill(
        CREATE_PROFILE,
        &[
            ("%%PROFILES%%", quote(profiles_dir)),
            ("%%NAME%%", quote(name)),
            ("%%HOMES%%", quote(homes_root)),
            (
                "%%USER_DEFAULT%%",
                if user_default { "true" } else { "false" }.to_string(),
            ),
        ],
    )
}

pub fn home_path_script(path: &str) -> String {
    fill(
        HOME_PATH,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATH%%", quote(path)),
        ],
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

pub fn profile_packages_script(profile_dir: &str) -> String {
    fill(
        PROFILE_PACKAGES,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATH%%", quote(profile_dir)),
        ],
    )
}

pub async fn ensure_home_path(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    path: &str,
) -> Result<String, HostError> {
    let answer: HomePath = json(
        transport,
        host,
        home_path_script(path),
        DEFAULT_TIMEOUT,
        Shell::Plain,
    )
    .await?;
    Ok(answer.path)
}

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
    json(
        transport,
        host,
        user_home_script(),
        DEFAULT_TIMEOUT,
        Shell::Plain,
    )
    .await
}

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteProfilePackages {
    pub manifest: Option<Vec<u8>>,
    pub installed: std::collections::BTreeMap<String, String>,
}

const READ_FILES: &str = r#"set -u
set -f
%%ESCAPER%%
dsh_paths=%%PATHS%%
dsh_rows=
dsh_sep=
dsh_old_ifs=$IFS
IFS='
'
for dsh_path in $dsh_paths; do
  if [ -f "$dsh_path" ]; then
    dsh_value="\"$(base64 < "$dsh_path" | tr -d '\n')\""
  else
    dsh_value=null
  fi
  dsh_rows="$dsh_rows$dsh_sep$dsh_value"
  dsh_sep=,
done
IFS=$dsh_old_ifs
printf '<<<DSH-JSON>>>{"files":[%s]}<<<DSH-END>>>' "$dsh_rows"
"#;

pub async fn read_files(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    paths: &[String],
) -> Result<Vec<Option<Vec<u8>>>, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        files: Vec<Option<String>>,
    }
    let out = transport
        .run(host, RemoteCall::new(read_files_script(paths)))
        .await?;
    let answer: Answer = parse_marked(&out)?;
    answer
        .files
        .into_iter()
        .map(|value| match value {
            None => Ok(None),
            Some(encoded) => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(encoded.as_bytes())
                    .map(Some)
                    .map_err(|e| {
                        HostError::remote_command(format!("那台机器上的文件内容不是 base64：{e}"))
                    })
            }
        })
        .collect()
}

pub fn read_files_script(paths: &[String]) -> String {
    fill(
        READ_FILES,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PATHS%%", quote(&paths.join("\n"))),
        ],
    )
}

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

const READ_FILE: &str = r#"set -u
dsh_path=%%PATH%%
if [ ! -f "$dsh_path" ]; then
  printf '远端没有这个文件：%s\n' "$dsh_path" >&2
  exit 2
fi
dsh_data=$(base64 < "$dsh_path" | tr -d '\n')
printf '<<<DSH-JSON>>>{"data":"%s"}<<<DSH-END>>>' "$dsh_data"
"#;

const WRITE_FILE: &str = r#"set -u
dsh_path=%%PATH%%
mkdir -p "$(dirname "$dsh_path")" || exit 2
printf %s %%BASE64%% | base64 -d > "$dsh_path" || exit 3
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

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

const STOP_PID: &str = r#"set -u
kill -TERM %%PID%% 2>/dev/null || { printf '不能给进程 %s 发信号\n' %%PID%% >&2; exit 3; }
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

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

const ALIVE: &str = r#"set -u
dsh_pid=%%PID%%
if kill -0 -- "-$dsh_pid" 2>/dev/null || kill -0 "$dsh_pid" 2>/dev/null; then
  printf '<<<DSH-JSON>>>{"alive":true}<<<DSH-END>>>'
else
  printf '<<<DSH-JSON>>>{"alive":false}<<<DSH-END>>>'
fi
"#;

const PORT_OWNER: &str = r#"set -u
dsh_port=%%PORT%%
dsh_ss=$(command -v ss 2>/dev/null || true)
if [ -z "$dsh_ss" ] && [ -x /usr/sbin/ss ]; then dsh_ss=/usr/sbin/ss; fi
if [ -z "$dsh_ss" ] && [ -x /sbin/ss ]; then dsh_ss=/sbin/ss; fi
if [ -z "$dsh_ss" ]; then
  printf '<<<DSH-JSON>>>{"occupied":false,"pid":null,"command":"","args":"","error":"ss-not-found"}<<<DSH-END>>>'
  exit 0
fi
dsh_ss_out=$("$dsh_ss" -ltnp 2>/dev/null)
dsh_ss_status=$?
if [ "$dsh_ss_status" -ne 0 ]; then
  printf '<<<DSH-JSON>>>{"occupied":false,"pid":null,"command":"","args":"","error":"ss-failed"}<<<DSH-END>>>'
  exit 0
fi
dsh_row=$(printf '%s\n' "$dsh_ss_out" | grep -E ":${dsh_port}[[:space:]]" | head -n 1)
if [ -z "$dsh_row" ]; then
  printf '<<<DSH-JSON>>>{"occupied":false,"pid":null,"command":"","args":""}<<<DSH-END>>>'
  exit 0
fi
dsh_pid=$(printf '%s' "$dsh_row" | grep -o 'pid=[0-9]*' | head -n 1 | cut -d= -f2)
if [ -z "$dsh_pid" ]; then
  printf '<<<DSH-JSON>>>{"occupied":true,"pid":null,"command":"","args":""}<<<DSH-END>>>'
  exit 0
fi
dsh_name=$(ps -p "$dsh_pid" -o comm= 2>/dev/null | head -n 1 | tr -d '\r\n"\\')
dsh_args=$(ps -p "$dsh_pid" -o args= 2>/dev/null | head -n 1 | tr -d '\r\n"\\')
printf '<<<DSH-JSON>>>{"occupied":true,"pid":%s,"command":"%s","args":"%s"}<<<DSH-END>>>' \
  "$dsh_pid" "$dsh_name" "$dsh_args"
"#;

const STOP_RUN: &str = r#"set -u
dsh_pid=%%PID%%
dsh_started=%%STARTED%%
dsh_wait=%%WAIT%%

dsh_alive() {
  kill -0 -- "-$dsh_pid" 2>/dev/null || kill -0 "$dsh_pid" 2>/dev/null
}

dsh_now() {
  dsh_stat=$(cat "/proc/$dsh_pid/stat" 2>/dev/null || true)
  dsh_rest=${dsh_stat##*') '}
  dsh_value=$(printf '%s' "$dsh_rest" | awk '{print $20}' 2>/dev/null || true)
  case "$dsh_value" in
    ''|*[!0-9]*) printf 'null' ;;
    *) printf '%s' "$dsh_value" ;;
  esac
}

dsh_answer() {
  printf '<<<DSH-JSON>>>{"outcome":"%s","started_at":%s}<<<DSH-END>>>' "$1" "$(dsh_now)"
}

dsh_current=$(dsh_now)
if [ "$dsh_current" = null ]; then
  # The pid has no /proc entry: either it is gone, or this user cannot read it.
  if dsh_alive; then dsh_answer unreadable; else dsh_answer gone; fi
  exit 0
fi
if [ "$dsh_started" = null ]; then dsh_answer unprepared; exit 0; fi
if [ "$dsh_current" != "$dsh_started" ]; then dsh_answer reused; exit 0; fi

# The whole group: a detached spawn runs under `setsid`, so its pid leads the
# group and the signal reaches what the instance started too.
if ! { kill -TERM -- "-$dsh_pid" 2>/dev/null || kill -TERM "$dsh_pid" 2>/dev/null; }; then
  if dsh_alive; then dsh_answer refused; else dsh_answer gone; fi
  exit 0
fi

dsh_ticks=$((dsh_wait * 10))
dsh_i=0
while [ "$dsh_i" -lt "$dsh_ticks" ] && dsh_alive; do
  sleep 0.1
  dsh_i=$((dsh_i + 1))
done
if dsh_alive; then dsh_answer lingering; else dsh_answer stopped; fi
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    Stopped,
    Gone,
    Reused {
        now: Option<u64>,
        recorded: Option<u64>,
    },
    Unreadable,
    Unprepared,
    Refused,
    Lingering,
}

pub async fn stop_run(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    pid: i32,
    started_at: Option<u64>,
    wait_secs: u64,
) -> Result<StopOutcome, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        outcome: String,
        started_at: Option<u64>,
    }
    let out = transport
        .run(
            host,
            RemoteCall::within(
                stop_run_script(pid, started_at, wait_secs),
                budget_for(wait_secs),
            ),
        )
        .await?;
    let answer: Answer = parse_marked(&out)?;
    Ok(match answer.outcome.as_str() {
        "stopped" => StopOutcome::Stopped,
        "gone" => StopOutcome::Gone,
        "reused" => StopOutcome::Reused {
            now: answer.started_at,
            recorded: started_at,
        },
        "unreadable" => StopOutcome::Unreadable,
        "unprepared" => StopOutcome::Unprepared,
        "refused" => StopOutcome::Refused,
        "lingering" => StopOutcome::Lingering,
        other => {
            return Err(HostError::remote_command(format!(
                "那台机器给的停止结果是未知的：{other}"
            )))
        }
    })
}

pub fn stop_run_script(pid: i32, started_at: Option<u64>, wait_secs: u64) -> String {
    fill(
        STOP_RUN,
        &[
            ("%%PID%%", pid.to_string()),
            (
                "%%STARTED%%",
                started_at
                    .map(|at| at.to_string())
                    .unwrap_or_else(|| "null".to_string()),
            ),
            ("%%WAIT%%", wait_secs.to_string()),
        ],
    )
}

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

const HELLO: &str = "true";

pub fn hello_script() -> String {
    HELLO.to_string()
}

const LOG_SIZE: &str = r#"set -u
dsh_bytes=$(wc -c < %%LOG%% 2>/dev/null | tr -d ' ')
if [ -z "$dsh_bytes" ]; then dsh_bytes=0; fi
printf '<<<DSH-JSON>>>{"offset":%s}<<<DSH-END>>>' "$dsh_bytes"
"#;

const FILE_READY: &str = r#"set -u
if [ -s %%PATH%% ]; then dsh_ready=true; else dsh_ready=false; fi
printf '<<<DSH-JSON>>>{"ready":%s}<<<DSH-END>>>' "$dsh_ready"
"#;

const STDOUT_TAIL: &str = r#"set -u
%%ESCAPER%%
dsh_pid=%%PID%%
dsh_bytes=%%BYTES%%
dsh_target=$(readlink "/proc/$dsh_pid/fd/1" 2>/dev/null || true)
if [ -z "$dsh_target" ] || [ ! -f "$dsh_target" ]; then
  printf '<<<DSH-JSON>>>{"path":null,"data":""}<<<DSH-END>>>'
  exit 0
fi
dsh_data=$(tail -c "$dsh_bytes" "$dsh_target" 2>/dev/null | base64 | tr -d '\n')
dsh_quoted=$(dsh_esc "$dsh_target")
printf '<<<DSH-JSON>>>{"path":"%s","data":"%s"}<<<DSH-END>>>' "$dsh_quoted" "$dsh_data"
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdoutTail {
    pub path: Option<String>,
    pub text: String,
}

pub async fn stdout_tail(
    transport: &dyn HostTransport,
    host: &RemoteHost,
    pid: i32,
    max_bytes: usize,
) -> Result<StdoutTail, HostError> {
    #[derive(Deserialize)]
    struct Answer {
        path: Option<String>,
        data: String,
    }
    let out = transport
        .run(host, RemoteCall::new(stdout_tail_script(pid, max_bytes)))
        .await?;
    let answer: Answer = parse_marked(&out)?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(answer.data.as_bytes())
        .map_err(|e| HostError::remote_command(format!("远端输出无法解码：{e}")))?;
    Ok(StdoutTail {
        path: answer.path,
        text: String::from_utf8_lossy(&bytes).to_string(),
    })
}

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

const TAIL: &str = r#"set -u
dsh_log=%%LOG%%
dsh_offset=%%OFFSET%%
dsh_pid=%%PID%%
dsh_wait=%%WAIT%%
dsh_max=%%MAX%%

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

dsh_end=$(dsh_size)
dsh_from=$((dsh_offset + 1))
if [ "$dsh_end" -gt "$dsh_offset" ] && [ $((dsh_end - dsh_offset)) -gt "$dsh_max" ]; then
  dsh_from=$((dsh_end - dsh_max + 1))
fi
dsh_data=$(tail -c "+$dsh_from" "$dsh_log" 2>/dev/null | head -c "$dsh_max" | base64 | tr -d '\n')
if dsh_alive; then dsh_living=true; else dsh_living=false; fi
printf '<<<DSH-JSON>>>{"offset":%s,"data":"%s","alive":%s}<<<DSH-END>>>' "$dsh_end" "$dsh_data" "$dsh_living"
"#;

const REMOVE_DIR: &str = r#"set -u
dsh_path=%%PATH%%
rm -rf -- "$dsh_path" || { printf '删不掉远端目录 %s\n' "$dsh_path" >&2; exit 2; }
printf '<<<DSH-JSON>>>{"ok":true}<<<DSH-END>>>'
"#;

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

const SPAWN: &str = r#"set -u
dsh_log=%%LOG%%
mkdir -p "$(dirname "$dsh_log")" || exit 2
%%PATH_PREPEND%%
nohup setsid %%ARGV%% >> "$dsh_log" 2>&1 < /dev/null &
printf '<<<DSH-JSON>>>{"pid":%s}<<<DSH-END>>>' "$!"
"#;

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

pub fn tail_script(log_path: &str, offset: u64, pid: i32, wait_secs: u64) -> String {
    fill(
        TAIL,
        &[
            ("%%LOG%%", quote(log_path)),
            ("%%OFFSET%%", offset.to_string()),
            ("%%PID%%", pid.to_string()),
            ("%%WAIT%%", wait_secs.to_string()),
            ("%%MAX%%", MAX_TAIL_BYTES.to_string()),
        ],
    )
}

pub fn log_size_script(log_path: &str) -> String {
    fill(LOG_SIZE, &[("%%LOG%%", quote(log_path))])
}

pub fn file_ready_script(path: &str) -> String {
    fill(FILE_READY, &[("%%PATH%%", quote(path))])
}

pub fn start_time_script(pid: i32) -> String {
    fill(START_TIME, &[("%%PID%%", pid.to_string())])
}

pub fn stdout_tail_script(pid: i32, max_bytes: usize) -> String {
    fill(
        STDOUT_TAIL,
        &[
            ("%%ESCAPER%%", ESCAPER.to_string()),
            ("%%PID%%", pid.to_string()),
            ("%%BYTES%%", max_bytes.to_string()),
        ],
    )
}

pub fn port_owner_script(port: u16) -> String {
    fill(PORT_OWNER, &[("%%PORT%%", port.to_string())])
}

pub fn mint_script(url: &str) -> String {
    fill(MINT_REMOTE, &[("%%URL%%", quote(url))])
}

pub fn spawn_script(argv: &[String], log_path: &str, path_dirs: &[String]) -> String {
    let command = argv
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    let path_prepend = if path_dirs.is_empty() {
        String::new()
    } else {
        let joined = path_dirs
            .iter()
            .map(|dir| quote(dir))
            .collect::<Vec<_>>()
            .join(":");
        format!("export PATH={joined}:\"$PATH\"")
    };
    fill(
        SPAWN,
        &[
            ("%%LOG%%", quote(log_path)),
            ("%%ARGV%%", command),
            ("%%PATH_PREPEND%%", path_prepend),
        ],
    )
}

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

pub fn exit_status_of(line: &str) -> Option<i32> {
    let rest = line.trim().strip_prefix(EXIT_MARK)?;
    rest.strip_suffix(END)?.trim().parse().ok()
}

pub fn terminal_script(home: &str, env: &[(String, String)], path_dirs: &[String]) -> String {
    let mut script = String::new();
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

pub fn terminal_command(target: &str, script: &str) -> String {
    format!("ssh -t {target} {}", quote(script))
}

pub const LOG_TAIL_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogTail {
    pub text: String,
    pub exists: bool,
}

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

pub fn log_tail_script(log_path: &str, max_bytes: usize) -> String {
    fill(
        LOG_TAIL,
        &[
            ("%%LOG%%", quote(log_path)),
            ("%%BYTES%%", max_bytes.to_string()),
        ],
    )
}

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
    #[serde(default)]
    link: RemotePathLink,
}

#[derive(Deserialize)]
struct Pid {
    pid: i32,
}

#[derive(Deserialize)]
struct Ack {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
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

pub fn decode_remote_versions(out: &RemoteOutput) -> Result<RemoteVersions, HostError> {
    let answer = parse_marked::<Installed>(out)?;
    Ok(RemoteVersions {
        versions: answer.versions,
        link: answer.link,
    })
}

pub fn decode_pid(out: &RemoteOutput) -> Result<i32, HostError> {
    Ok(parse_marked::<Pid>(out)?.pid)
}

pub fn decode_ack(out: &RemoteOutput) -> Result<(), HostError> {
    let ack = parse_marked::<Ack>(out)?;
    if ack.ok {
        return Ok(());
    }
    Err(HostError::remote_command(
        ack.error
            .filter(|why| !why.trim().is_empty())
            .unwrap_or_else(|| "远端拒绝了这次写入".to_string()),
    ))
}

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

pub fn decode_profile_packages(out: &RemoteOutput) -> Result<RemoteProfilePackages, HostError> {
    use base64::Engine as _;
    let answer: ProfilePackages = parse_marked(out)?;
    let manifest = answer.manifest.and_then(|data| {
        base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .ok()
    });
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

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::ssh::{HostFuture, PortForward};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    pub(crate) fn marked(payload: &str) -> String {
        format!("{}{}{}", super::MARK, payload, super::END)
    }

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
        pub(crate) fn stdout(stdout: impl Into<String>) -> Self {
            Self::Output {
                status: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            }
        }

        pub(crate) fn failed(status: i32, stderr: impl Into<String>) -> Self {
            Self::Output {
                status,
                stdout: String::new(),
                stderr: stderr.into(),
            }
        }

        pub(crate) fn fail(kind: HostErrorKind, message: &str) -> Self {
            Self::Fail(HostError::new(kind, message))
        }
    }

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

        pub(crate) fn answering(self: &Arc<Self>, answer: FakeAnswer) -> Arc<Self> {
            self.answers.lock().unwrap().push_back(answer);
            self.clone()
        }

        pub(crate) fn calls(&self) -> Vec<FakeCall> {
            self.calls.lock().unwrap().clone()
        }

        pub(crate) fn last_script(&self) -> String {
            match self.calls.lock().unwrap().last() {
                Some(FakeCall::Run(script)) | Some(FakeCall::Spawn(script)) => script.clone(),
                _ => String::new(),
            }
        }

        fn next(&self) -> Result<FakeAnswer, HostError> {
            self.answers.lock().unwrap().pop_front().ok_or_else(|| {
                HostError::unreachable("测试没有给这次调用排好应答（FakeTransport）")
            })
        }

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

    pub(crate) fn idle() -> Arc<dyn HostTransport> {
        FakeTransport::new()
    }

    pub(crate) fn connection_stdout(
        node: Option<(&str, &str)>,
        pnpm: Option<(&str, &str)>,
    ) -> String {
        connection_stdout_with_dir("/home/demo/.local/share/dsh-launcher", node, pnpm)
    }

    pub(crate) fn connection_stdout_with_dir(
        data_dir: &str,
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
             \"home\":\"/home/demo\",\"data_dir\":\"{data_dir}\",\"data_dir_problem\":null,\
             \"data_dir_exists\":false,\"node\":{},\"pnpm\":{}}}",
            tool(node),
            tool(pnpm)
        ))
    }

    pub(crate) fn file_stdout(contents: &[u8]) -> String {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(contents);
        marked(&format!("{{\"data\":\"{data}\"}}"))
    }

    pub(crate) fn files_stdout(files: &[Option<&[u8]>]) -> String {
        use base64::Engine as _;
        let rows = files
            .iter()
            .map(|file| match file {
                Some(contents) => format!(
                    "\"{}\"",
                    base64::engine::general_purpose::STANDARD.encode(contents)
                ),
                None => "null".to_string(),
            })
            .collect::<Vec<_>>()
            .join(",");
        marked(&format!("{{\"files\":[{rows}]}}"))
    }

    pub(crate) fn tail_stdout(text: &str, offset: u64) -> String {
        tail_stdout_with(text, offset, true)
    }

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

    pub(crate) fn tail_stdout_after_exit(text: &str, offset: u64) -> String {
        tail_stdout_with(text, offset, false)
    }

    pub(crate) fn prime_home_answers(ready_url: &str) -> Vec<FakeAnswer> {
        vec![
            FakeAnswer::stdout(marked(r#"{"profiles":[]}"#)),
            FakeAnswer::stdout(marked(r#"{"ready":true}"#)),
            FakeAnswer::stdout(environment_stdout(
                remote_tool("/home/linuxbrew/.linuxbrew/bin/node", "v26.4.0", "system"),
                remote_tool_missing(vec![]),
                &[],
            )),
            FakeAnswer::stdout(marked(r#"{"offset":0}"#)),
            FakeAnswer::Pid(PRIME_PID),
            FakeAnswer::stdout(marked(r#"{"started_at":117384720}"#)),
            FakeAnswer::stdout(tail_stdout(&format!("dsh web: {ready_url}\n"), 320)),
            FakeAnswer::stdout(marked(r#"{"outcome":"stopped","started_at":117384720}"#)),
            FakeAnswer::stdout(marked(r#"{"ok":true}"#)),
            FakeAnswer::stdout(file_stdout(b"- id: webserver\n  port: 20017\n")),
            FakeAnswer::Done,
        ]
    }

    pub(crate) const PRIME_PID: i32 = 5150;

    pub(crate) const PRIME_PINNED_PORT: &str = "20017";

    fn tail_stdout_with(text: &str, offset: u64, alive: bool) -> String {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        marked(&format!(
            "{{\"offset\":{offset},\"data\":\"{data}\",\"alive\":{alive}}}"
        ))
    }

    pub(crate) fn remote_tool(path: &str, version: &str, source: &str) -> serde_json::Value {
        serde_json::json!({
            "path": path,
            "version": version,
            "source": source,
            "skipped": [],
        })
    }

    pub(crate) fn remote_tool_missing(skipped: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({
            "path": null,
            "version": null,
            "source": null,
            "skipped": skipped,
        })
    }

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
            path_dirs: &'a [String],
        ) -> HostFuture<'a, Result<i32, HostError>> {
            let script = spawn_script(argv, log_path, path_dirs);
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

    #[test]
    fn a_line_that_never_ends_is_cut_at_the_buffer_cap() {
        let mut pending = String::from("完整的一行\n");
        pending.push_str(&"x".repeat(MAX_LOG_LINE_BYTES + 10));
        let mut lines = Vec::new();

        assert!(
            take_log_lines(&mut pending, &mut lines),
            "超长行要被切掉并说出来"
        );
        assert_eq!(
            lines,
            vec!["完整的一行".to_string(), "x".repeat(CUT_LINE_PREFIX_BYTES)]
        );
        assert!(pending.is_empty(), "超长行的剩余部分不在缓冲里留着");

        let mut pending = String::from("半行");
        let mut lines = Vec::new();
        assert!(!take_log_lines(&mut pending, &mut lines));
        assert!(lines.is_empty());
        assert_eq!(pending, "半行", "没到上限的半行留着等下一次读取");
    }

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
        assert_eq!(
            test.pnpm.as_ref().map(|t| t.path.as_str()),
            Some("/home/demo/.local/share/pnpm/pnpm")
        );
    }

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
            let fake = FakeTransport::new().answering(FakeAnswer::fail(kind, message));
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
        assert!(
            !said.contains("no job control"),
            "登录 shell 的废话不是失败原因：{said}"
        );
    }

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
        assert!(
            script.contains(MARK) && script.contains(END),
            "脚本必须用与解析端同一对标记：{script}"
        );
    }

    #[test]
    fn a_path_with_spaces_and_quotes_survives_a_real_shell() {
        let path = "/home/me/a b/it's 'quoted'/file.txt";
        let script = read_file_script(path);

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

        let write = write_file_script(path, b"hello");
        let assignment = write
            .lines()
            .find(|line| line.starts_with("dsh_path="))
            .unwrap();
        assert!(assignment.ends_with("'/home/me/a b/it'\\''s '\\''quoted'\\''/file.txt'"));
    }

    #[test]
    fn the_connection_test_script_answers_parsable_json_on_a_real_shell() {
        let output = local_shell(&connection_test_script(None));

        let facts: Facts = parse_marked(&output).expect("真实 shell 跑出来的答案要能解析");

        assert!(!facts.os.is_empty() && !facts.release.is_empty() && !facts.arch.is_empty());
        assert_eq!(facts.home, std::env::var("HOME").unwrap_or_default());
        let data_dir = facts.data_dir.expect("默认数据目录要能 settle 出来");
        assert!(
            data_dir.ends_with("/.local/share/dsh-launcher"),
            "默认数据目录按那台机器的家目录推：{data_dir}"
        );
        assert_eq!(
            facts.data_dir_exists,
            std::path::Path::new(&data_dir).is_dir(),
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

    #[test]
    fn the_typed_data_directory_is_settled_on_that_machine() {
        let home = std::env::var("HOME").unwrap_or_default();
        let cases = [
            ("~/dsh", format!("{home}/dsh")),
            ("~/dsh/", format!("{home}/dsh")),
            ("~/a/../dsh", format!("{home}/dsh")),
            ("~", home.clone()),
            ("/srv//dsh/", "/srv/dsh".to_string()),
        ];
        for (typed, expected) in cases {
            let output = local_shell(&connection_test_script(Some(typed)));
            let facts: Facts = parse_marked(&output).expect("真实 shell 跑出来的答案要能解析");
            assert_eq!(
                facts.data_dir.as_deref(),
                Some(expected.as_str()),
                "{typed} 要 settle 成绝对路径"
            );
        }
    }

    #[test]
    fn a_data_directory_that_cannot_be_settled_comes_back_as_a_reason() {
        for (typed, expected) in [("dsh", "绝对路径"), ("~bob/dsh", "~用户名")] {
            let output = local_shell(&connection_test_script(Some(typed)));
            assert!(
                output.stdout.contains("\"data_dir\":null"),
                "{typed} 不能当路径用：{}",
                output.stdout
            );
            assert!(
                output.stdout.contains(expected),
                "{typed} 的理由要说清楚：{}",
                output.stdout
            );
            let facts: Facts = parse_marked(&output).expect("真实 shell 跑出来的答案要能解析");
            assert!(facts.data_dir.is_none(), "{typed}: {:?}", facts.data_dir);
            assert!(
                facts
                    .data_dir_problem
                    .unwrap_or_default()
                    .contains(expected),
                "{typed}: 解析出来的理由"
            );
        }
        let settled = local_shell(&connection_test_script(Some("~/dsh")));
        assert!(
            settled.stdout.contains("\"data_dir_problem\":null"),
            "settle 得了的就没有理由可说：{}",
            settled.stdout
        );
    }

    #[test]
    fn the_installed_versions_script_answers_each_versions_path_and_bin_readiness() {
        let home = scratch("dsh-versions-home");
        let scratch = std::env::temp_dir().join(format!("dsh-versions-{}", uuid::Uuid::new_v4()));
        let versions = scratch.join("versions");
        let npm_tree = versions.join("0.1.0-rc.6");
        std::fs::create_dir_all(npm_tree.join("node_modules/@deepseek-ai/dsh/lib")).unwrap();
        std::fs::write(
            npm_tree.join("node_modules/@deepseek-ai/dsh/lib/bin.js"),
            "// dsh\n",
        )
        .unwrap();
        let checkout = versions.join("0.1.0-rc.5");
        std::fs::create_dir_all(checkout.join("apps/cli/lib")).unwrap();
        std::fs::write(checkout.join("apps/cli/package.json"), "{}").unwrap();
        std::fs::write(checkout.join("apps/cli/lib/bin.js"), "// dsh\n").unwrap();
        std::fs::create_dir_all(versions.join("0.1.0-rc.4")).unwrap();
        let link = home.join(".local/bin/dsh");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(npm_tree.join("node_modules/.bin/dsh"), &link).unwrap();

        let known = [npm_tree.to_string_lossy().to_string()];
        let known: Vec<&str> = known.iter().map(String::as_str).collect();
        let out = local_shell_as(
            &installed_versions_script(&versions.to_string_lossy(), &known),
            &home,
        );
        let answer = decode_remote_versions(&out).expect("真实 shell 跑出来的答案要能解析");
        let found = answer.versions;

        let by_name = |name: &str| found.iter().find(|v| v.name == name).unwrap().clone();
        assert_eq!(found.len(), 3);
        assert_eq!(by_name("0.1.0-rc.6").dir, npm_tree.to_string_lossy());
        assert!(by_name("0.1.0-rc.6").bin_ready, "装好的版本 bin 就绪");
        assert_eq!(by_name("0.1.0-rc.5").dir, checkout.to_string_lossy());
        assert!(by_name("0.1.0-rc.5").bin_ready, "repo checkout 也是就绪");
        assert!(!by_name("0.1.0-rc.4").bin_ready, "bin 没写出来就不是就绪");
        assert_eq!(answer.link.state, "symlink");
        assert_eq!(
            answer.link.target,
            npm_tree.join("node_modules/.bin/dsh").to_string_lossy()
        );

        std::fs::remove_dir_all(&scratch).ok();
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_remote_bin_check_uses_the_local_launchers_two_layouts() {
        let script = installed_versions_script("/opt/dsh/versions", &[]);
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

    #[test]
    fn the_path_link_script_registers_replaces_and_withdraws_on_that_machine() {
        let home = scratch("dsh-path-link-home");
        let versions = scratch("dsh-path-link-versions");
        let first = versions.join("0.2.1-rc.1");
        let second = versions.join("0.2.2-rc.1");
        for dir in [&first, &second] {
            let bin = crate::launch::version_bin(dir);
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, "#!/usr/bin/env node\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let dirs = [
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ];
        let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
        let link = home.join(".local/bin/dsh");

        let run = |action: &str, version: Option<&str>| {
            local_shell_as(&path_link_script(action, version, &dirs), &home)
        };

        let out = run("register", Some(&first.to_string_lossy()));
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            crate::launch::version_bin(&first)
        );

        let listed = local_shell_as(
            &installed_versions_script(&versions.to_string_lossy(), &dirs),
            &home,
        );
        let answer = decode_remote_versions(&listed).unwrap();
        assert_eq!(answer.link.state, "symlink");
        assert_eq!(
            answer.link.target,
            crate::launch::version_bin(&first).to_string_lossy()
        );

        let out = run("register", Some(&second.to_string_lossy()));
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            crate::launch::version_bin(&second)
        );

        let out = run("unlink", Some(&first.to_string_lossy()));
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(link.exists(), "别的版本的链接不动");

        let out = run("unlink", Some(&second.to_string_lossy()));
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(!link.exists(), "撤掉自己的链接");

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&versions).ok();
    }

    #[test]
    fn the_path_link_script_leaves_a_users_own_dsh_alone() {
        let home = scratch("dsh-path-link-foreign");
        let versions = scratch("dsh-path-link-foreign-versions");
        let version = versions.join("0.2.1-rc.1");
        let bin = crate::launch::version_bin(&version);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/usr/bin/env node\n").unwrap();
        let link = home.join(".local/bin/dsh");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::fs::write(&link, "#!/bin/sh\necho system dsh\n").unwrap();

        let dirs = [version.to_string_lossy().to_string()];
        let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
        let out = local_shell_as(
            &path_link_script("register", Some(&version.to_string_lossy()), &dirs),
            &home,
        );
        assert_ne!(out.status, 0, "用户自己的 dsh 不被覆盖");
        assert_eq!(
            std::fs::read_to_string(&link).unwrap(),
            "#!/bin/sh\necho system dsh\n"
        );

        let out = local_shell_as(&path_link_script("unregister", None, &dirs), &home);
        assert_ne!(out.status, 0, "用户自己的 dsh 不被删除");
        assert!(link.exists());

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&versions).ok();
    }

    #[test]
    fn the_create_profile_script_picks_the_template_the_way_the_local_flow_does() {
        let root = scratch("dsh-create-profile");
        let home = root.join("homes/0.2.1-rc.1");
        let profiles = home.join("profiles");
        std::fs::create_dir_all(profiles.join("__temp__")).unwrap();
        std::fs::write(profiles.join("__temp__/package.json"), "{}").unwrap();

        let out = local_shell_as(
            &create_profile_script(
                &profiles.to_string_lossy(),
                "lab-x",
                &root.join("homes").to_string_lossy(),
                false,
            ),
            &root,
        );
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert_eq!(
            std::fs::read_to_string(profiles.join("lab-x/package.json")).unwrap(),
            "{}"
        );

        let other = root.join("homes/0.2.2-rc.1/profiles");
        std::fs::create_dir_all(other.join("web")).unwrap();
        std::fs::write(
            other.join("web/cordis.patch.yml"),
            "- id: webserver\n  port: 8080\n",
        )
        .unwrap();
        let out = local_shell_as(
            &create_profile_script(
                &other.to_string_lossy(),
                "web-copy",
                &root.join("homes").to_string_lossy(),
                false,
            ),
            &root,
        );
        assert_eq!(out.status, 0, "{}", out.stderr);
        let patch = std::fs::read_to_string(other.join("web-copy/cordis.patch.yml")).unwrap();
        assert!(
            patch.contains("port: 0"),
            "复制件里的端口固定值要清掉：{patch}"
        );

        let bare = root.join("homes/0.2.3-rc.1/profiles");
        std::fs::create_dir_all(&bare).unwrap();
        let out = local_shell_as(
            &create_profile_script(
                &bare.to_string_lossy(),
                "borrowed",
                &root.join("homes").to_string_lossy(),
                false,
            ),
            &root,
        );
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(bare.join("borrowed").is_dir());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_path_link_script_wraps_a_cli_without_an_executable_bit() {
        let home = scratch("dsh-path-link-wrap-home");
        let versions = scratch("dsh-path-link-wrap-versions");
        let version = versions.join("0.2.1-rc.1");
        let bin = crate::launch::version_bin(&version);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/usr/bin/env node\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let version_text = version.to_string_lossy().to_string();
        let dirs = [version_text.clone()];
        let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
        let link = home.join(".local/bin/dsh");

        let out = local_shell_as(
            &path_link_script("register", Some(&version_text), &dirs),
            &home,
        );
        assert_eq!(out.status, 0, "{}", out.stderr);

        let body = std::fs::read_to_string(&link).unwrap();
        assert!(
            body.starts_with("#!/bin/sh\n"),
            "包装脚本的第一行必须是 shebang：{body}"
        );
        assert!(
            body.contains(crate::pathlink::MARKER.trim()),
            "包装脚本要带标记，否则以后撤不掉：{body}"
        );
        assert!(
            body.contains(&version_text),
            "包装脚本要写明它跑的是哪个版本目录：{body}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&link).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "包装脚本必须可执行");
        }

        let listed = local_shell_as(
            &installed_versions_script(&versions.to_string_lossy(), &dirs),
            &home,
        );
        let answer = decode_remote_versions(&listed).unwrap();
        assert_eq!(answer.link.state, "script", "{}", listed.stdout);
        assert_eq!(answer.link.target, version_text);

        let out = local_shell_as(&path_link_script("unregister", None, &dirs), &home);
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(!link.exists(), "取消登记要撤掉启动器自己的包装脚本");
    }

    #[test]
    fn the_path_link_script_withdraws_only_the_deleted_versions_wrapper() {
        let home = scratch("dsh-path-link-unlink-home");
        let versions = scratch("dsh-path-link-unlink-versions");
        let first = versions.join("0.2.1-rc.1");
        let second = versions.join("0.2.2-rc.1");
        for dir in [&first, &second] {
            let bin = crate::launch::version_bin(dir);
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, "#!/usr/bin/env node\n").unwrap();
        }
        let first_text = first.to_string_lossy().to_string();
        let second_text = second.to_string_lossy().to_string();
        let dirs = [first_text.clone(), second_text.clone()];
        let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
        let link = home.join(".local/bin/dsh");

        let out = local_shell_as(
            &path_link_script("register", Some(&second_text), &dirs),
            &home,
        );
        assert_eq!(out.status, 0, "{}", out.stderr);

        let out = local_shell_as(&path_link_script("unlink", Some(&first_text), &dirs), &home);
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(link.exists(), "别的版本的登记不动");

        let out = local_shell_as(
            &path_link_script("unlink", Some(&second_text), &dirs),
            &home,
        );
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(!link.exists(), "删掉的那个版本自己的登记要撤掉");

        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/usr/bin/true", &link).unwrap();
        let out = local_shell_as(&path_link_script("unlink", None, &dirs), &home);
        assert_eq!(out.status, 0, "{}", out.stderr);
        assert!(link.exists(), "没指定版本时不能删掉别人的链接");
    }

    #[test]
    fn the_path_link_script_treats_an_unknown_marker_as_the_users_own() {
        let home = scratch("dsh-path-link-stray-home");
        let versions = scratch("dsh-path-link-stray-versions");
        let version = versions.join("0.2.1-rc.1");
        let bin = crate::launch::version_bin(&version);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/usr/bin/env node\n").unwrap();
        let version_text = version.to_string_lossy().to_string();
        let dirs = [version_text.clone()];
        let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
        let link = home.join(".local/bin/dsh");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let stray = format!(
            "#!/bin/sh\n{} /opt/somewhere/else\n",
            crate::pathlink::MARKER.trim()
        );
        std::fs::write(&link, &stray).unwrap();

        let out = local_shell_as(
            &path_link_script("register", Some(&version_text), &dirs),
            &home,
        );
        assert_ne!(out.status, 0, "指着未知目录的标记不是启动器的，不能覆盖");
        assert_eq!(std::fs::read_to_string(&link).unwrap(), stray);

        let out = local_shell_as(&path_link_script("unregister", None, &dirs), &home);
        assert_ne!(out.status, 0, "不是启动器的，不能删");
        assert_eq!(std::fs::read_to_string(&link).unwrap(), stray);

        let listed = local_shell_as(
            &installed_versions_script(&versions.to_string_lossy(), &dirs),
            &home,
        );
        let answer = decode_remote_versions(&listed).unwrap();
        assert_eq!(answer.link.state, "foreign", "{}", listed.stdout);
    }

    #[test]
    fn the_versions_query_reaches_the_machine_quoted() {
        let script = available_versions_script("https://npm.example.com/it's mirror");
        assert!(
            script.contains("--registry 'https://npm.example.com/it'\\''s mirror'"),
            "{script}"
        );
    }

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

    #[test]
    fn an_unreachable_machine_fails_both_queries() {
        for answer in [
            FakeAnswer::fail(HostErrorKind::Unreachable, "连接被拒绝"),
            FakeAnswer::failed(2, "npm: command not found"),
        ] {
            let fake = FakeTransport::new().answering(answer);
            let transport: &dyn HostTransport = &*fake;

            let versions = block_on(installed_versions(
                transport,
                &host(),
                "/opt/dsh/versions",
                &[],
            ));
            let available = block_on(available_versions(
                transport,
                &host(),
                "https://npm.example.com",
            ));

            assert!(versions.is_err(), "{versions:?}");
            assert!(available.is_err(), "{available:?}");
        }
    }

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
                &[],
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
        assert!(
            spawned.contains("< /dev/null"),
            "脱离会话的进程要断掉 stdin：{spawned}"
        );
        assert!(
            spawned.contains("'/usr/bin/node' '/opt/dsh/bin.js'"),
            "argv 逐段引用，调用处不拼 shell 字符串：{spawned}"
        );
        assert_eq!(calls[3], FakeCall::StopPid(4242));
        assert_eq!(
            calls[4],
            FakeCall::WriteFile("/home/demo/.dsh/icons/i-1.png".into(), vec![1, 2, 3])
        );
    }

    #[test]
    fn a_detached_spawn_prepends_the_given_directories_to_path() {
        let argv = vec!["/usr/bin/node".to_string()];
        let plain = spawn_script(&argv, "/tmp/log", &[]);
        assert!(!plain.contains("export PATH="), "{plain}");

        let with = spawn_script(
            &argv,
            "/tmp/log",
            &[
                "/home/demo/.local/share/pnpm".to_string(),
                "/opt/node/bin".to_string(),
            ],
        );
        assert!(
            with.contains("export PATH='/home/demo/.local/share/pnpm':'/opt/node/bin':\"$PATH\""),
            "{with}"
        );
    }

    #[test]
    fn the_real_transport_would_dial_with_the_script_built_here() {
        let host = host();
        let script = connection_test_script(None);
        let argv = crate::ssh::ssh_argv(
            &host.target,
            &host.socket,
            &crate::ssh::script_command(&script, Shell::Login),
        );
        assert_eq!(argv.last().unwrap(), &quote(&script));
        assert_eq!(argv[argv.len() - 3], crate::ssh::REMOTE_SHELL);
        assert_eq!(argv[argv.len() - 2], "-ilc");
        let _ = SshTransport::new();
    }

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
        assert!(
            env.node.skipped.is_empty(),
            "没有低于门槛的候选就不该报跳过"
        );

        let env = run_probe(&home, &path, Some(picked.to_str().unwrap()), &fallback);
        assert_eq!(
            env.node.path.as_deref(),
            Some(picked.to_str().unwrap()),
            "用户指定的排在系统环境变量前面"
        );
        assert_eq!(env.node.source.as_deref(), Some("manual"));
    }

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

        let env = block_on(environment(
            transport,
            &host(),
            Some("/opt/pick/node"),
            None,
        ))
        .unwrap();

        assert_eq!(env.shell, "/bin/bash");
        assert_eq!(env.node.source.as_deref(), Some("system"));
        assert_eq!(env.pnpm.path, None);
        assert_eq!(env.pnpm.skipped[0].reason, "below-minimum");
        assert_eq!(env.managers, vec!["nvm".to_string()]);

        let script = fake.last_script();
        assert!(script.contains("'/opt/pick/node'"), "{script}");
        assert!(
            script.contains("$HOME/.nvm/versions/node/*/bin"),
            "{script}"
        );
        assert!(script.contains("$HOME/.local/share/pnpm"), "{script}");
    }

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
            assert!(
                script.contains("--version"),
                "校验方式就是跑一次 --version：{script}"
            );
        }
    }

    #[test]
    fn an_install_command_survives_both_shells_on_the_way_to_the_machine() {
        let command = ssh_command("lab", "echo one two | tr a-z A-Z");
        assert!(command.starts_with("ssh lab "), "{command}");
        assert!(
            ssh_command("user@example.com -p 2222", "echo hi")
                .starts_with("ssh user@example.com -p 2222 "),
            "自由目标（带自己的参数）也要能用"
        );

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

    #[test]
    fn a_command_quoting_a_path_survives_both_shells() {
        let path = "/home/demo/o'brien dir";
        let command = ssh_command(TARGET, &format!("printf %s {}", quote(path)));
        assert!(command.starts_with("ssh zjq_aliyun "), "{command}");

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

    #[test]
    fn the_install_recipes_extend_what_the_machine_already_has() {
        assert_eq!(
            node_install_recipe(&["nvm".to_string()]),
            "nvm install --lts"
        );
        assert_eq!(
            node_install_recipe(&["fnm".to_string()]),
            "fnm install --lts"
        );
        assert_eq!(
            node_install_recipe(&["brew".to_string()]),
            "brew install node"
        );
        let bare = node_install_recipe(&[]);
        assert!(
            bare.contains("install.sh") && bare.contains("nvm.sh") && bare.contains("nvm install --lts"),
            "没有管理器时要先装 nvm 再在同一个 shell 里加载它（新的 ~/.bashrc 只在下一次生效）：{bare}"
        );
        assert!(pnpm_install_recipe().contains("get.pnpm.io"));

        for recipe in [
            node_install_recipe(&[]),
            node_install_recipe(&["nvm".to_string()]),
            node_install_recipe(&["brew".to_string()]),
            pnpm_install_recipe(),
        ] {
            assert!(!recipe.contains('"'), "配方里不能有双引号：{recipe}");
        }
    }

    fn shell_alive(pid: i32) -> bool {
        let out = local_shell(&alive_script(pid));
        parse_marked::<serde_json::Value>(&out).unwrap()["alive"]
            .as_bool()
            .unwrap()
    }

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

    fn wait_gone(pid: i32) -> bool {
        for _ in 0..20 {
            if !pid_exists(pid) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

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

    #[test]
    fn stopping_something_already_gone_is_not_an_error() {
        let out = local_shell(&stop_group_script(999_999));
        decode_ack(&out).expect("本来就不在了也算停好了");
    }

    #[test]
    fn the_single_pid_stop_still_signals_only_that_pid() {
        let script = stop_pid_script(4242);
        assert!(script.contains("kill -TERM 4242"), "{script}");
        assert!(
            !script.contains("-4242"),
            "不该用进程组的形式（那会连带停掉别人的孩子）：{script}"
        );
    }

    #[test]
    fn a_detached_command_writes_its_exit_status_into_its_own_log() {
        let dir = scratch("dsh-measured");
        let target = dir.join("tree");

        let argv = measured_argv(
            &[
                "/usr/bin/env".to_string(),
                "CI=true".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo out; echo err >&2; exit 3".to_string(),
            ],
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

        assert!(
            target.is_dir(),
            "工作目录要在那里被建出来：{}",
            target.display()
        );
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

    #[test]
    fn a_tail_read_returns_only_what_was_appended_since_the_last_one() {
        let dir = scratch("dsh-tail");
        let log = dir.join("install.log");
        std::fs::write(&log, "line one\n").unwrap();
        let path = log.to_string_lossy().to_string();

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

        let quick = std::time::Instant::now();
        let out = local_shell(&tail_script(&path, 18, 999_999, 30));
        let parsed: serde_json::Value = parse_marked(&out).unwrap();
        assert!(!parsed["alive"].as_bool().unwrap());
        assert!(quick.elapsed().as_secs() < 5, "进程没了就不该再等");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_tail_read_that_was_left_behind_returns_only_the_end_of_the_log() {
        let dir = scratch("dsh-tail-behind");
        let log = dir.join("install.log");
        let head = "x".repeat(MAX_TAIL_BYTES);
        let tail = "line at the end\n";
        std::fs::write(&log, format!("{head}{tail}")).unwrap();
        let size = std::fs::metadata(&log).unwrap().len();
        let path = log.to_string_lossy().to_string();

        let out = local_shell(&tail_script(&path, 0, 0, 0));
        let parsed: serde_json::Value = parse_marked(&out).unwrap();
        assert_eq!(
            parsed["offset"], size,
            "offset 落在文件末尾，下一轮从那里接着读"
        );
        let read = base64::engine::general_purpose::STANDARD
            .decode(parsed["data"].as_str().unwrap())
            .unwrap();
        let text = String::from_utf8_lossy(&read);
        assert_eq!(text.len(), MAX_TAIL_BYTES, "一次读取有上限：{}", text.len());
        assert!(text.ends_with(tail), "读到的是尾部：{text:?}");

        let again = local_shell(&tail_script(&path, size, 0, 0));
        let parsed: serde_json::Value = parse_marked(&again).unwrap();
        assert_eq!(parsed["offset"], size);
        assert_eq!(parsed["data"], "");

        std::fs::remove_dir_all(&dir).ok();
    }

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

        let again = local_shell(&remove_dir_script(&nested.to_string_lossy()));
        decode_ack(&again).expect("本来就不在了也算删好了");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_version_lookup_asks_that_machines_own_npm() {
        let with_registry = npm_has_version_script(
            "@deepseek-ai/dsh@1.2.3",
            Some("https://mirror.example/it's"),
        );
        assert!(with_registry.contains("command -v npm"), "{with_registry}");
        assert!(
            with_registry.contains("'@deepseek-ai/dsh@1.2.3'"),
            "{with_registry}"
        );
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
        assert!(
            fake.last_script().contains(&path),
            "问的是那台机器上的这个日志"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

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
            web.join("node_modules")
                .join("@scope")
                .join("one")
                .join("package.json"),
            "{\n  \"name\": \"@scope/one\",\n  \"version\": \"3.1.4\"\n}\n",
        )
        .unwrap();
        std::fs::write(
            web.join("node_modules").join("plain").join("package.json"),
            r#"{"name":"plain","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            web.join("node_modules")
                .join("versionless")
                .join("package.json"),
            r#"{"name":"versionless"}"#,
        )
        .unwrap();

        let out = local_shell(&profile_packages_script(&web.to_string_lossy()));
        let files = decode_profile_packages(&out).expect("一次调用读回整个 Profile");

        assert_eq!(files.manifest.as_deref(), Some(manifest.as_bytes()));
        assert_eq!(
            files.installed.get("@scope/one").map(String::as_str),
            Some("3.1.4")
        );
        assert_eq!(
            files.installed.get("plain").map(String::as_str),
            Some("1.0.0")
        );
        assert!(
            !files.installed.contains_key("versionless"),
            "清单里没有版本就不报版本：{:?}",
            files.installed
        );

        let absent = local_shell(&profile_packages_script(
            &dir.join("nope").to_string_lossy(),
        ));
        let files = decode_profile_packages(&absent).unwrap();
        assert!(files.manifest.is_none());
        assert!(files.installed.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

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
        assert!(
            fake.last_script().contains(&path),
            "问的是那台机器上的这个文件"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_start_time_script_separates_a_live_pid_from_one_nothing_owns() {
        #[derive(serde::Deserialize)]
        struct Started {
            started_at: Option<u64>,
        }
        let gone: Started = parse_marked(&local_shell(&start_time_script(4_000_000))).unwrap();
        assert_eq!(gone.started_at, None, "没人占用的 pid 没有启动时间");

        let fake = FakeTransport::new()
            .answering(FakeAnswer::stdout(marked(r#"{"started_at":117384720}"#)));
        let started = block_on(process_start_time(&*fake, &host(), 4242)).unwrap();
        assert_eq!(started, Some(117384720));
        assert!(
            fake.last_script().contains("dsh_pid=4242"),
            "读的是那个 pid 自己的记录"
        );
    }

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

    #[test]
    fn the_home_script_settles_the_path_by_text_without_following_links() {
        let home = scratch("dsh-remote-settle");
        let raw = format!("{0}/a//../a/b/", home.to_string_lossy());

        let out = local_shell_as(&home_path_script(&raw), &home);
        let resolved: HomePath = parse_marked(&out).expect("真实 shell 的答案要能解析");

        assert_eq!(resolved.path, home.join("a/b").to_string_lossy());
        assert_eq!(
            decode_names(&local_shell(&list_dir_script(&home.to_string_lossy())))
                .unwrap()
                .len(),
            1,
            "只留下一层 a"
        );

        std::fs::remove_dir_all(&home).ok();
    }

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

        let out = local_shell_as(&home_path_script("~someone/.dsh"), &home);
        assert_ne!(out.status, 0);
        assert!(out.stderr.contains("~用户名"), "{}", out.stderr);

        std::fs::remove_dir_all(&home).ok();
    }

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
        let out = local_shell(&profiles_script(&empty.join("profiles").to_string_lossy()));
        assert!(decode_profiles(&out).unwrap().is_empty());

        std::fs::remove_dir_all(&home).ok();
    }

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

        let out = local_shell(&copy_profile_script(&from, &to));
        assert_ne!(out.status, 0);
        assert!(out.stderr.contains("远端已经有这个目录"), "{}", out.stderr);
        let missing = profiles.join("nope").to_string_lossy().to_string();
        let out = local_shell(&copy_profile_script(&missing, &to));
        assert_ne!(out.status, 0);
        assert!(
            out.stderr.contains("远端没有这个 Profile 目录"),
            "{}",
            out.stderr
        );

        let renamed = profiles.join("renamed").to_string_lossy().to_string();
        decode_ack(&local_shell(&rename_profile_script(&to, &renamed))).unwrap();
        assert!(profiles.join("renamed").is_dir());
        assert!(!profiles.join("web copy").exists());

        decode_ack(&local_shell(&delete_profile_script(&renamed))).unwrap();
        assert!(!profiles.join("renamed").exists());
        assert!(profiles.join("web").is_dir(), "删一个不动另一个");

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_terminal_command_lands_in_the_homes_directory_through_both_shells() {
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
        let path_dirs = vec![
            "/home/demo/.local/share/dsh-launcher/versions/0.1.0/node_modules/.bin".to_string(),
        ];
        let script = terminal_script(&home, &env, &path_dirs);
        let command = terminal_command(TARGET, &script);
        assert!(command.starts_with("ssh -t zjq_aliyun "), "{command}");

        assert!(!script.contains("@@@"), "分隔符不能出现在脚本里：{script}");
        let handed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s@@@' {command}"))
            .output()
            .unwrap();
        let handed_text = String::from_utf8_lossy(&handed.stdout).to_string();
        let mut words: Vec<String> = handed_text.split("@@@").map(str::to_string).collect();
        assert_eq!(
            words.pop().as_deref(),
            Some(""),
            "最后一个 @@ 后面不该有东西"
        );
        assert_eq!(words[0], "ssh");
        assert_eq!(words[1], "-t", "远端会话要有终端，否则 shell 不是交互的");
        assert_eq!(words[2], TARGET);
        assert_eq!(
            words.len(),
            4,
            "ssh 只该拿到一个参数（脚本可能被换行拆开了）"
        );

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
            vec![home.clone(), home.clone(), "服务器 实例".to_string()],
            "远端会话要 cd 进那个目录，并带上这两个变量：{:?} / {:?} / {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            out.status
        );
        assert!(
            script.contains(&format!("export PATH={}:", quote(path_dirs[0].as_str()))),
            "版本的 bin 目录要排在最前（远端不写 wrapper，pnpm 自己的 .bin 就是入口）：{script}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

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

        let out = local_shell(&log_tail_script(
            &dir.join("nothing.log").to_string_lossy(),
            40,
        ));
        let missing = decode_log_tail(&out).unwrap();
        assert!(!missing.exists);
        assert!(missing.text.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

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

        let dir = format!("/tmp/dsh-profile-real-{}", uuid::Uuid::new_v4().simple());
        let web = format!("{dir}/profiles/web");
        let manifest = r#"{"dsh":{"profile":{"bundles":["plain"]}},"dependencies":{"plain":"^1"}}"#;
        transport
            .write_file(
                &host,
                &format!("{web}/package.json"),
                manifest.as_bytes().to_vec(),
            )
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
        assert_eq!(
            files.installed.get("plain").map(String::as_str),
            Some("1.2.3")
        );
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
