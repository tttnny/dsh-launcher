//! The one seam that knows ssh.
//!
//! Every remote action the launcher takes — running a script, starting a
//! detached process, stopping it by pid or as a whole process tree, reading or
//! writing a file, listing a directory, opening a port forward — goes through
//! [`HostTransport`]. That
//! keeps "how we talk to the other machine" in one place (ADR-0006), and it is
//! what lets a test drive the whole remote feature through an in-memory double
//! instead of a live server. The shape is copied from `native::NativeTransport`,
//! the launcher's other process/network seam.
//!
//! Two rules hold here:
//!
//! * **Shell text never appears in this module.** Every script comes from
//!   [`crate::remote`]; a call site (including this one) never assembles a
//!   shell string of its own.
//! * **The launcher dials through its own socket.** `~/.ssh/config` may set
//!   `ControlPath` for the user's own terminals; sharing it would let the
//!   launcher and the user's shells tear down each other's connections, so the
//!   launcher keeps its own under its data directory.

use crate::config::Host;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

/// The launcher's own `ssh`. Absolute, so it does not depend on the `PATH` this
/// GUI app happened to inherit.
pub const SSH_BIN: &str = "/usr/bin/ssh";

/// The shell every remote script runs in: interactive *and* login, so it reads
/// what the user's terminal reads (`~/.bashrc` is where Linux users put their
/// `PATH`). Probing and executing use this same shape, which is the only way
/// the page and the work can agree on which Node is meant (ADR-0005 追加).
pub const REMOTE_SHELL: &str = "/bin/bash";

/// How long ssh may take to establish a connection.
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// How long the launcher's multiplexed connection survives its last use.
const CONTROL_PERSIST_SECS: u64 = 60;

/// ssh's own failure status. A remote command that exits 255 is
/// indistinguishable from ssh failing to connect — a limit of the protocol, so
/// 255 is read as the transport's word.
const SSH_FAILURE_STATUS: i32 = 255;

/// How long a whole remote call may take unless the caller says otherwise. A
/// login shell on the other side has to start (rc files, plugin managers)
/// before it answers anything, and the ssh handshake itself may spend up to
/// [`CONNECT_TIMEOUT_SECS`] on a machine that is slow to answer.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A boxed future, so [`HostTransport`] stays object-safe (`Arc<dyn …>`) the
/// way `native::NativeFuture` does.
pub type HostFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Failure classes
// ---------------------------------------------------------------------------

/// The four ways a remote call fails, told apart because the user's next move
/// differs: 连不上 (network), 认证失败 (keys), 超时 (slow or hung), 远端命令失败
/// (their script, carrying what it said).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostErrorKind {
    /// ssh never reached the machine: refused, unresolvable, no route.
    Unreachable,
    /// The machine answered but would not let us in — no key and no agent to
    /// offer one, or a host key `known_hosts` does not know yet.
    Auth,
    /// No answer in time: a remote call that hung, or ssh's connect timeout.
    Timeout,
    /// We got in, the command ran, and it failed (or said nothing the launcher
    /// could read). The message carries the tail of its stderr.
    RemoteCommand,
}

impl HostErrorKind {
    /// The word the page shows for this class.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unreachable => "连不上",
            Self::Auth => "认证失败",
            Self::Timeout => "超时",
            Self::RemoteCommand => "远端命令失败",
        }
    }
}

/// Why one remote call did not produce an answer: the class, plus one readable
/// sentence for the user. The sentence carries what the remote side said (its
/// stderr tail) and, where the class has one, the thing the user should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError {
    pub kind: HostErrorKind,
    pub message: String,
}

impl HostError {
    pub fn new(kind: HostErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::new(HostErrorKind::Unreachable, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(HostErrorKind::Auth, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(HostErrorKind::Timeout, message)
    }

    pub fn remote_command(message: impl Into<String>) -> Self {
        Self::new(HostErrorKind::RemoteCommand, message)
    }

    pub fn label(&self) -> &'static str {
        self.kind.label()
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}：{}", self.label(), self.message)
    }
}

// ---------------------------------------------------------------------------
// The transport
// ---------------------------------------------------------------------------

/// One ready-made remote invocation: a script built by [`crate::remote`], and
/// how long it may take. Call sites never assemble `script` themselves.
#[derive(Clone, Debug)]
pub struct RemoteCall {
    pub script: String,
    pub timeout: Duration,
}

impl RemoteCall {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn within(script: impl Into<String>, timeout: Duration) -> Self {
        Self {
            script: script.into(),
            timeout,
        }
    }
}

/// What one remote script left behind. A non-zero `status` is **not** an error
/// here: a script that reports 「这台机器没有 pnpm」 exits 0 and answers, and one
/// that fails outright is read by [`crate::remote`] — which owns the marker
/// convention — before it becomes a [`HostError`].
#[derive(Clone, Debug, Default)]
pub struct RemoteOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// One host as the transport sees it: which machine to dial, through which
/// socket. Built from a [`Host`] record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHost {
    pub id: String,
    /// The ssh target as the user typed it: an alias from `~/.ssh/config`, or a
    /// free string such as `user@host -p 2222`.
    pub target: String,
    /// The launcher's own connection-multiplexing socket for this target.
    pub socket: PathBuf,
}

impl RemoteHost {
    /// The ssh way to reach `host`. A built-in host is this Mac — nothing is
    /// dialed for it, so it has no address and nothing to test.
    pub fn of(host: &Host, launcher_data_dir: &Path) -> Result<Self, HostError> {
        let target = host.ssh_target.trim();
        if host.builtin || target.is_empty() {
            return Err(HostError::unreachable(format!(
                "「{}」是本机：它就在这台 Mac 上，没有需要连接的机器",
                host.display_name
            )));
        }
        Ok(Self {
            id: host.id.clone(),
            target: target.to_string(),
            socket: socket_path(launcher_data_dir, target),
        })
    }
}

/// One local→remote port forward (ADR-0008). The caller chooses the local port:
/// the launcher prefers the remote one and falls back to a free local port,
/// which only the caller can know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortForward {
    pub local_port: u16,
    /// The address as seen *from the host itself* (`127.0.0.1`).
    pub remote_host: String,
    pub remote_port: u16,
}

/// The `-L` spec for one forward: `[bind_address:]port:host:hostport`.
///
/// The bind address is spelled out as loopback rather than left implicit. A
/// bare `port:host:port` is bound according to `GatewayPorts`, which a user's
/// own `~/.ssh/config` may leave on — and an instance's address offered to this
/// Mac's network is exactly what the whole feature refuses to do (ADR-0008).
/// The remote end travels verbatim: the address DSH printed from its own point
/// of view is the address the forward has to reach.
pub fn forward_spec(forward: &PortForward) -> String {
    format!(
        "127.0.0.1:{}:{}:{}",
        forward.local_port, forward.remote_host, forward.remote_port
    )
}

/// The argv words of one `-O` control request on an **established** connection:
/// `forward`/`cancel` carry the `-L` spec, `exit` ends the connection and needs
/// nothing. ssh does not dial a machine for `-O`; it talks to the master that
/// owns the launcher's socket, which is why these calls fail instantly when
/// there is none.
fn control_command(action: &str, forward: Option<&PortForward>) -> Vec<String> {
    let mut argv = vec!["-O".to_string(), action.to_string()];
    if let Some(forward) = forward {
        argv.push("-L".to_string());
        argv.push(forward_spec(forward));
    }
    argv
}

/// The one boundary every remote action crosses, injectable so a test
/// substitutes an in-memory double. Production is [`SshTransport`].
///
/// The whole method list is declared here so the tickets that need a
/// capability later add no seam of their own. [`run`](Self::run) is what the
/// connection test exercises; the rest are the same plumbing, and the module's
/// tests drive each one through the fake to keep it honest.
pub trait HostTransport: Send + Sync {
    /// Runs one script in a fresh remote login shell and returns what it left
    /// behind.
    fn run<'a>(
        &'a self,
        host: &'a RemoteHost,
        call: RemoteCall,
    ) -> HostFuture<'a, Result<RemoteOutput, HostError>>;

    /// Starts one session-detached process (ADR-0007: it must outlive the ssh
    /// session) and returns its pid. `argv` is the command line as the launcher
    /// would run it locally; the detached-start script is [`crate::remote`]'s,
    /// never the caller's.
    fn spawn_detached<'a>(
        &'a self,
        host: &'a RemoteHost,
        argv: &'a [String],
        log_path: &'a str,
    ) -> HostFuture<'a, Result<i32, HostError>>;

    /// Sends SIGTERM to one pid on that host.
    fn stop_pid<'a>(
        &'a self,
        host: &'a RemoteHost,
        pid: i32,
    ) -> HostFuture<'a, Result<(), HostError>>;

    /// Stops the whole process tree one [`spawn_detached`](Self::spawn_detached)
    /// started: the process group `pid` leads, falling back to `pid` itself when
    /// it leads none. Already gone counts as stopped.
    ///
    /// A detached spawn runs under `setsid`, so its pid is also its process
    /// group's id and a negative pid reaches every child it started. That is the
    /// difference between cancelling an install and leaving a pnpm helper behind
    /// holding the store's lock (issue #9 风险).
    fn stop_group<'a>(
        &'a self,
        host: &'a RemoteHost,
        pid: i32,
    ) -> HostFuture<'a, Result<(), HostError>>;

    /// Reads one file's bytes.
    fn read_file<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
    ) -> HostFuture<'a, Result<Vec<u8>, HostError>>;

    /// Writes one file's bytes, creating its directory.
    fn write_file<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
        contents: Vec<u8>,
    ) -> HostFuture<'a, Result<(), HostError>>;

    /// Lists one directory's entry names.
    fn list_dir<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
    ) -> HostFuture<'a, Result<Vec<String>, HostError>>;

    /// Opens a port forward to that host.
    fn open_forward<'a>(
        &'a self,
        host: &'a RemoteHost,
        forward: PortForward,
    ) -> HostFuture<'a, Result<(), HostError>>;

    /// Closes a forward [`open_forward`](Self::open_forward) opened.
    fn close_forward<'a>(
        &'a self,
        host: &'a RemoteHost,
        forward: PortForward,
    ) -> HostFuture<'a, Result<(), HostError>>;

    /// Ends the launcher's own multiplexed connection to that host, and with it
    /// every forward that was riding it. The user's own connections use their
    /// own socket; nothing of theirs is touched. A host with no connection up
    /// is already in the state this asks for.
    fn close_connection<'a>(
        &'a self,
        host: &'a RemoteHost,
    ) -> HostFuture<'a, Result<(), HostError>>;
}

/// The real transport: the system `ssh`, through the launcher's own control
/// socket. Stateless — a host carries its own address.
#[derive(Default)]
pub struct SshTransport;

impl SshTransport {
    pub fn new() -> Self {
        Self
    }

    /// The one place a local ssh process is spawned.
    async fn dial(
        &self,
        host: &RemoteHost,
        command: &[String],
        timeout: Duration,
    ) -> Result<RemoteOutput, HostError> {
        if let Some(dir) = host.socket.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                HostError::unreachable(format!(
                    "无法建立启动器的 ssh 连接目录 {}：{e}",
                    dir.display()
                ))
            })?;
        }
        let argv = ssh_argv(&host.target, &host.socket, command);
        let mut cmd = tokio::process::Command::new(SSH_BIN);
        cmd.args(&argv);
        // No terminal in a GUI app: ssh must not try to ask anything, and a
        // remote command must not be able to read this app's stdin.
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // A hung ssh must not outlive the call that started it.
        cmd.kill_on_drop(true);

        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(HostError::unreachable(format!("无法启动 {SSH_BIN}：{e}"))),
            Err(_) => {
                return Err(HostError::timeout(format!(
                    "ssh {} 在 {} 秒内没有返回",
                    host.target,
                    timeout.as_secs()
                )))
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let status = output.status.code().unwrap_or(SSH_FAILURE_STATUS);
        if status == SSH_FAILURE_STATUS {
            return Err(ssh_failure(&host.target, &stderr));
        }
        Ok(RemoteOutput {
            status,
            stdout,
            stderr,
        })
    }

    async fn script(
        &self,
        host: &RemoteHost,
        call: RemoteCall,
    ) -> Result<RemoteOutput, HostError> {
        let command = script_command(&call.script);
        self.dial(host, &command, call.timeout).await
    }

    /// Adds or cancels one `-L` forward on the launcher's own multiplexed
    /// connection. That connection is already up whenever the launcher has
    /// talked to the host, which every other call here ensures by using the
    /// same socket — **and which adding a forward has to ensure itself**: `ssh
    /// -O forward` is a request to a master, it does not start one, so a
    /// launcher that has not talked to the host yet would be refused the first
    /// time the user opens an instance.
    async fn forward(
        &self,
        host: &RemoteHost,
        action: &str,
        forward: &PortForward,
    ) -> Result<(), HostError> {
        if action == "forward" {
            self.script(host, RemoteCall::new(crate::remote::hello_script()))
                .await?;
        }
        let command = control_command(action, Some(forward));
        self.dial(host, &command, CONTROL_TIMEOUT).await.map(|_| ())
    }

    /// Ends the launcher's own connection to one host, which takes every forward
    /// riding it with it. Nothing is dialed to do it: no master means there is
    /// nothing of ours left on that machine, which is the state this call is
    /// asking for. The user's own connections live on their own socket and are
    /// not touched.
    async fn end_connection(&self, host: &RemoteHost) -> Result<(), HostError> {
        self.dial(host, &control_command("exit", None), CONTROL_TIMEOUT)
            .await
            .map(|_| ())
    }
}

/// How long an `-O` control request (a forward, its cancellation) may take; it
/// rides an established connection and only has to be acknowledged.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

impl HostTransport for SshTransport {
    fn run<'a>(
        &'a self,
        host: &'a RemoteHost,
        call: RemoteCall,
    ) -> HostFuture<'a, Result<RemoteOutput, HostError>> {
        Box::pin(async move { self.script(host, call).await })
    }

    fn spawn_detached<'a>(
        &'a self,
        host: &'a RemoteHost,
        argv: &'a [String],
        log_path: &'a str,
    ) -> HostFuture<'a, Result<i32, HostError>> {
        Box::pin(async move {
            let call = RemoteCall::new(crate::remote::spawn_script(argv, log_path));
            let out = self.script(host, call).await?;
            crate::remote::decode_pid(&out)
        })
    }

    fn stop_pid<'a>(
        &'a self,
        host: &'a RemoteHost,
        pid: i32,
    ) -> HostFuture<'a, Result<(), HostError>> {
        Box::pin(async move {
            let call = RemoteCall::new(crate::remote::stop_pid_script(pid));
            let out = self.script(host, call).await?;
            crate::remote::decode_ack(&out)
        })
    }

    fn stop_group<'a>(
        &'a self,
        host: &'a RemoteHost,
        pid: i32,
    ) -> HostFuture<'a, Result<(), HostError>> {
        Box::pin(async move {
            let call = RemoteCall::new(crate::remote::stop_group_script(pid));
            let out = self.script(host, call).await?;
            crate::remote::decode_ack(&out)
        })
    }

    fn read_file<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
    ) -> HostFuture<'a, Result<Vec<u8>, HostError>> {
        Box::pin(async move {
            let call = RemoteCall::new(crate::remote::read_file_script(path));
            let out = self.script(host, call).await?;
            crate::remote::decode_blob(&out)
        })
    }

    fn write_file<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
        contents: Vec<u8>,
    ) -> HostFuture<'a, Result<(), HostError>> {
        Box::pin(async move {
            let call = RemoteCall::new(crate::remote::write_file_script(path, &contents));
            let out = self.script(host, call).await?;
            crate::remote::decode_ack(&out)
        })
    }

    fn list_dir<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
    ) -> HostFuture<'a, Result<Vec<String>, HostError>> {
        Box::pin(async move {
            let call = RemoteCall::new(crate::remote::list_dir_script(path));
            let out = self.script(host, call).await?;
            crate::remote::decode_names(&out)
        })
    }

    fn open_forward<'a>(
        &'a self,
        host: &'a RemoteHost,
        forward: PortForward,
    ) -> HostFuture<'a, Result<(), HostError>> {
        Box::pin(async move { self.forward(host, "forward", &forward).await })
    }

    fn close_forward<'a>(
        &'a self,
        host: &'a RemoteHost,
        forward: PortForward,
    ) -> HostFuture<'a, Result<(), HostError>> {
        Box::pin(async move { self.forward(host, "cancel", &forward).await })
    }

    fn close_connection<'a>(
        &'a self,
        host: &'a RemoteHost,
    ) -> HostFuture<'a, Result<(), HostError>> {
        Box::pin(async move { self.end_connection(host).await })
    }
}

// ---------------------------------------------------------------------------
// The ssh command line
// ---------------------------------------------------------------------------

/// One `-o` value. ssh parses `-o` values the way it parses config-file lines,
/// so a value with a space in it has to be quoted or ssh reads the rest as
/// extra arguments and refuses the whole call — and that is not hypothetical:
/// the launcher's own data directory on macOS is
/// `/Users/<name>/Library/Application Support/in.dsh-plug.dsh-launcher`.
fn config_value(value: &str) -> String {
    const PLAIN: &str =
        "-_./~:@+,=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    if value.chars().all(|c| PLAIN.contains(c)) {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// The argv one launcher ssh call is made with: our options, the target as
/// typed, then `command`.
///
/// ssh keeps reading options up to the first word that is not one — that is
/// where the remote command starts — so a free target such as
/// `user@host -p 2222` works here, and when the target and our options
/// disagree ours win, because ssh takes the first value it sees.
pub fn ssh_argv(target: &str, socket: &Path, command: &[String]) -> Vec<String> {
    let mut argv = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        format!(
            "ControlPath={}",
            config_value(&socket.display().to_string())
        ),
        "-o".to_string(),
        format!("ControlPersist={CONTROL_PERSIST_SECS}"),
        "-o".to_string(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
    ];
    argv.extend(target.split_whitespace().map(str::to_string));
    argv.extend(command.iter().cloned());
    argv
}

/// The remote command words for one script. The script must reach the remote
/// shell as **one** word: ssh joins the command words with spaces and the
/// remote shell splits them again, so the quoting has to survive that second
/// shell (see [`crate::remote::quote`]).
pub fn script_command(script: &str) -> Vec<String> {
    vec![
        REMOTE_SHELL.to_string(),
        "-ilc".to_string(),
        crate::remote::quote(script),
    ]
}

/// Where the launcher keeps its own multiplexed connection for one target.
///
/// The file name is a hash of the target rather than the target itself: a Unix
/// socket path is short, and the target is not (it may be a free string such as
/// `user@host -p 2222`).
///
/// **Short enough or somewhere else.** A Unix socket address holds 104 bytes and
/// ssh spends 17 of them before it even binds: it binds `<ControlPath>.<16
/// random characters>` first and hard-links that into place (OpenSSH's
/// `muxserver_listen`), so the launcher has **86** bytes to work with — and a
/// stock macOS data directory already derives 89
/// (`~/Library/Application Support/in.dsh-plug.dsh-launcher/ssh/<16 hex>.sock`),
/// which ssh refuses outright ("too long for Unix domain socket"). So the
/// launcher's own directory is used when the path fits there, and otherwise the
/// socket moves to a short directory of the launcher's own under `$HOME`, its
/// name derived from **both** the data directory and the target: the socket is
/// one ssh master, and two data directories sharing one would put two
/// configurations' commands on the same session.
fn socket_path(launcher_data_dir: &Path, target: &str) -> PathBuf {
    let digest = Sha256::digest(target.as_bytes());
    let short: String = digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect();

    let own = launcher_data_dir.join("ssh").join(format!("{short}.sock"));
    if own.as_os_str().len() <= MAX_SOCKET_PATH {
        return own;
    }

    // `$HOME` and not the temporary directory: this Mac's `$TMPDIR` is a long
    // `/var/folders/…` path that would overflow this budget just as the data
    // directory did, and `/tmp` is shared with every other user on the machine —
    // a control socket somebody else can create first is a connection somebody
    // else can answer on.
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if home.as_os_str().is_empty() {
        // No home to move into: keep the launcher's own directory and let ssh say
        // what is wrong with the path ([`socket_path_too_long`] turns that into a
        // sentence a user can act on).
        return own;
    }
    let scoped: String = Sha256::digest(launcher_data_dir.to_string_lossy().as_bytes())
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    home.join(SHORT_SOCKET_DIR)
        .join(format!("{scoped}-{short}.sock"))
}

/// The directory the socket moves into when the launcher's data directory
/// cannot hold one. Hidden, and the launcher's own: the user's `~/.ssh` is
/// theirs and carries their own `ControlPath`.
const SHORT_SOCKET_DIR: &str = ".dsh-launcher-ssh";

/// How many bytes a path handed to ssh as `ControlPath` may use.
///
/// Measured against this Mac's OpenSSH: 86 passes, 87 fails with
/// `unix_listener: path "…" too long for Unix domain socket`, because of the
/// 17-byte temporary name ssh binds first. See [`ssh_failure`] for the other
/// half of that rule — ssh also refuses anything of 104 bytes or more before it
/// starts.
const MAX_SOCKET_PATH: usize = 86;

/// Classifies one failed ssh call. ssh reports its own failures on stderr and
/// exits 255; there is no structured code to read, so the four classes come
/// from what it said.
pub fn ssh_failure(target: &str, stderr: &str) -> HostError {
    // The one failure that is not about the machine: ssh's own control socket
    // path. It has two spellings (ssh's pre-flight check and the bind that
    // follows), neither of which says anything a user can act on, and both of
    // which would otherwise be filed under 「连不上」 with advice about the
    // machine being off — which is exactly what it is not.
    if let Some(message) = socket_path_too_long(stderr) {
        return HostError::unreachable(message);
    }
    let kind = ssh_failure_kind(stderr);
    let said = match stderr_tail(stderr) {
        tail if tail.is_empty() => "ssh 没有说明原因".to_string(),
        tail => tail,
    };
    let message = match kind {
        // The first connection to a machine whose key is not in `known_hosts`
        // fails exactly like this, and it is the one failure the launcher
        // cannot fix from here: accepting a host key needs a terminal.
        HostErrorKind::Auth => format!(
            "{said}。启动器只支持免密连接（密钥或 ssh-agent）：请先在终端里跑一次 \
             ssh {target} 确认能直接登录，第一次连接要在那里接受主机密钥（密码与口令启动器不保存）"
        ),
        HostErrorKind::Timeout => format!(
            "{said}。连接 {target} 超过 {CONNECT_TIMEOUT_SECS} 秒没有回应：确认网络能到这台机器"
        ),
        HostErrorKind::Unreachable => {
            format!("{said}。确认 ssh {target} 这台机器开着、地址没写错")
        }
        HostErrorKind::RemoteCommand => said,
    };
    HostError::new(kind, message)
}

/// The readable reason for ssh saying the launcher's own socket path is too
/// long, or `None` when that is not what it said.
///
/// Two texts, one cause: `ControlPath too long ('…' >= 104 bytes)` is ssh's
/// check before it starts, and `unix_listener: path "…" too long for Unix
/// domain socket` is the bind failing on the temporary name it adds. Only the
/// first names a limit, and neither says the path belongs to the launcher.
fn socket_path_too_long(stderr: &str) -> Option<String> {
    let said = stderr.to_lowercase();
    if !said.contains("controlpath too long") && !said.contains("too long for unix domain socket") {
        return None;
    }
    Some(format!(
        "启动器自己那条 ssh 连接的控制 socket 路径太长了：一台机器上的 Unix socket 地址最多 103 字节，\
         ssh 还会先在它后面加 17 个字符再绑定，所以最多只能用 {MAX_SOCKET_PATH} 字节。\
         把启动器的数据目录（设置里的「数据目录」）换到更短的路径下，或者把家目录放到更短的位置，就能连上——\
         这与那台机器无关"
    ))
}

fn ssh_failure_kind(stderr: &str) -> HostErrorKind {
    let said = stderr.to_lowercase();
    const AUTH: [&str; 6] = [
        "permission denied",
        "host key verification failed",
        "remote host identification has changed",
        "too many authentication failures",
        "no supported authentication methods",
        "authentication failed",
    ];
    if AUTH.iter().any(|hint| said.contains(hint)) {
        return HostErrorKind::Auth;
    }
    if said.contains("timed out") || said.contains("timeout") {
        return HostErrorKind::Timeout;
    }
    HostErrorKind::Unreachable
}

/// The tail of a remote stderr: the last few lines that are not the remote
/// shell's own chatter. `bash -i` announces 「no job control in this shell」 on
/// every single call, and that line must never become the failure reason the
/// user reads.
pub(crate) fn stderr_tail(stderr: &str) -> String {
    const KEEP: usize = 3;
    const MAX_CHARS: usize = 400;
    let mut lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !shell_chatter(line))
        .collect();
    if lines.len() > KEEP {
        lines = lines.split_off(lines.len() - KEEP);
    }
    let mut tail = lines.join("；");
    if tail.chars().count() > MAX_CHARS {
        tail = tail.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    tail
}

fn shell_chatter(line: &str) -> bool {
    line.contains("no job control in this shell")
        || line.contains("cannot set terminal process group")
}

// ---------------------------------------------------------------------------
// The user's own ~/.ssh/config
// ---------------------------------------------------------------------------

/// One alias the user's `~/.ssh/config` defines, with as much of its target as
/// the picker needs to be useful. The launcher does not re-implement ssh's
/// configuration language: it offers the alias, and the alias *is* what gets
/// handed to ssh — keys, jump hosts and connection reuse stay where the user
/// wrote them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshAlias {
    pub alias: String,
    /// `HostName`, when the block names one. An alias that only sets options
    /// for its own name uses the alias as the host name.
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

/// The aliases in the user's `~/.ssh/config`, for the picker in the settings
/// page. A machine with no config file has no aliases — that is not an error
/// worth showing anyone.
pub fn ssh_aliases() -> Vec<SshAlias> {
    match std::env::var_os("HOME") {
        Some(home) => ssh_aliases_at(&PathBuf::from(home).join(".ssh").join("config")),
        None => Vec::new(),
    }
}

/// The aliases in one config file. A missing or unreadable file is an empty
/// list: the user can always type a free target instead.
pub fn ssh_aliases_at(path: &Path) -> Vec<SshAlias> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_ssh_config(&text),
        Err(_) => Vec::new(),
    }
}

/// Reads the `Host` blocks out of one `~/.ssh/config`.
///
/// Wildcard patterns are left out (`Host *` is a catch-all full of defaults, not
/// an alias anyone picks), and a `Match` block's keywords are ignored — they
/// apply conditionally, so they are not an alias's target either.
pub fn parse_ssh_config(text: &str) -> Vec<SshAlias> {
    let mut aliases: Vec<SshAlias> = Vec::new();
    // The aliases the current block's keyword lines belong to.
    let mut block: Vec<usize> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, value) = split_keyword(line);
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                block.clear();
                for pattern in value.split_whitespace() {
                    if is_wildcard(pattern) {
                        continue;
                    }
                    let slot = match aliases.iter().position(|a| a.alias == pattern) {
                        Some(index) => index,
                        None => {
                            aliases.push(SshAlias {
                                alias: pattern.to_string(),
                                host_name: None,
                                user: None,
                                port: None,
                            });
                            aliases.len() - 1
                        }
                    };
                    block.push(slot);
                }
            }
            "match" => block.clear(),
            "hostname" => {
                for &slot in &block {
                    if aliases[slot].host_name.is_none() {
                        aliases[slot].host_name = Some(value.to_string());
                    }
                }
            }
            "user" => {
                for &slot in &block {
                    if aliases[slot].user.is_none() {
                        aliases[slot].user = Some(value.to_string());
                    }
                }
            }
            "port" => {
                let port = value.parse::<u16>().ok();
                for &slot in &block {
                    if aliases[slot].port.is_none() {
                        aliases[slot].port = port;
                    }
                }
            }
            // Every other keyword (IdentityFile, ProxyJump, …) is ssh's
            // business: the launcher hands over the alias and ssh reads it.
            _ => {}
        }
    }
    aliases
}

/// `Keyword value`, `Keyword=value` and `Keyword = value` are all legal.
fn split_keyword(line: &str) -> (&str, &str) {
    let cut = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let value = line[cut..]
        .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
        .trim();
    (&line[..cut], unquote(value))
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
        .trim()
}

fn is_wildcard(pattern: &str) -> bool {
    pattern.contains(['*', '?', '!'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_host(target: &str) -> Host {
        Host {
            id: "host-lab".into(),
            display_name: "服务器".into(),
            ssh_target: target.into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: Some(PathBuf::from("/home/u/.local/share/dsh-launcher")),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    /// The flags the whole remote feature rests on: non-interactive (a GUI app
    /// has no terminal to type a password into), the launcher's **own**
    /// multiplexing socket, and a connect timeout so a dead machine cannot hang
    /// the page.
    #[test]
    fn ssh_argv_carries_batch_mode_and_the_launchers_own_socket() {
        // Short enough to sit inside the data directory, and with a space in it
        // — the case the `-o` quoting exists for.
        let dir = Path::new("/Users/me/Library/Application Support/dsh");
        let host = RemoteHost::of(&remote_host("zjq_aliyun"), dir).unwrap();
        let argv = ssh_argv(&host.target, &host.socket, &script_command("echo hi"));

        assert!(
            argv.contains(&"BatchMode=yes".to_string()),
            "必须非交互：{argv:?}"
        );
        assert!(
            argv.contains(&format!(
                "ControlPath=\"{}\"",
                host.socket.display()
            )),
            "socket 路径里有空格（macOS 的 Application Support），必须是加引号的 -o 值：{argv:?}"
        );
        assert!(argv.contains(&"ControlMaster=auto".to_string()));
        assert!(
            argv.iter().any(|a| a.starts_with("ControlPersist=")),
            "连接复用要有存活时间：{argv:?}"
        );
        assert!(
            argv.iter().any(|a| a.starts_with("ConnectTimeout=")),
            "要有连接超时：{argv:?}"
        );
        assert!(
            !host.socket.starts_with("/Users/me/.ssh"),
            "socket 是启动器自己的，不能碰用户配置里的那个：{}",
            host.socket.display()
        );
    }

    /// A Unix socket path holds 104 bytes, and ssh does not bind the
    /// `ControlPath` directly: it binds `<ControlPath>.<16 random characters>`
    /// first and hard-links that into place, so 17 of the 103 usable bytes are
    /// spoken for and the launcher has 86 to work with.
    ///
    /// A stock macOS data directory already spends 89 of them
    /// (`~/Library/Application Support/in.dsh-plug.dsh-launcher/ssh/<16 hex>.sock`),
    /// which ssh refuses — so a HOME that is merely longer than this one, let
    /// alone a data directory the user pointed somewhere deep, must still get a
    /// socket, or every remote action fails with ssh's own complaint.
    const SOCKET_BUDGET: usize = 86;

    fn socket_len(host: &RemoteHost) -> usize {
        host.socket.as_os_str().len()
    }

    #[test]
    fn a_long_data_directory_still_yields_a_socket_ssh_can_bind() {
        let long = Path::new(
            "/Users/averylongusername/Library/Application Support/in.dsh-plug.dsh-launcher",
        );
        let host = RemoteHost::of(&remote_host("zjq_aliyun"), long).unwrap();

        assert!(
            socket_len(&host) <= SOCKET_BUDGET,
            "ssh 绑不了这么长的控制 socket（{} 字节）：{}",
            socket_len(&host),
            host.socket.display()
        );
        assert!(
            !host.socket.starts_with("/Users/averylongusername/.ssh"),
            "换位置也不能换到用户自己的 ssh 目录里：{}",
            host.socket.display()
        );
    }

    /// The stock macOS data directory is the case that matters most: this path
    /// is what `app_data_dir()` produces on an ordinary account, and ssh refuses
    /// the 89-byte socket it used to derive from it.
    #[test]
    fn the_stock_macos_data_directory_also_gets_a_bindable_socket() {
        let stock = Path::new("/Users/me/Library/Application Support/in.dsh-plug.dsh-launcher");
        let host = RemoteHost::of(&remote_host("zjq_aliyun"), stock).unwrap();

        assert!(
            socket_len(&host) <= SOCKET_BUDGET,
            "{} 字节放不下 ssh 的控制 socket：{}",
            socket_len(&host),
            host.socket.display()
        );
    }

    /// Moving the socket out must not merge two launcher data directories onto
    /// one connection: the socket is the multiplexed ssh master, so sharing one
    /// would put two configurations' commands on the same session. The name is
    /// derived from the data directory as well as the target.
    #[test]
    fn two_data_directories_never_share_one_socket() {
        let one = RemoteHost::of(
            &remote_host("zjq_aliyun"),
            Path::new("/Users/averylongusername/one/in.dsh-plug.dsh-launcher"),
        )
        .unwrap();
        let two = RemoteHost::of(
            &remote_host("zjq_aliyun"),
            Path::new("/Users/averylongusername/two/in.dsh-plug.dsh-launcher"),
        )
        .unwrap();
        let again = RemoteHost::of(
            &remote_host("zjq_aliyun"),
            Path::new("/Users/averylongusername/one/in.dsh-plug.dsh-launcher"),
        )
        .unwrap();

        assert_ne!(one.socket, two.socket, "两个数据目录是两条连接");
        assert_eq!(one.socket, again.socket, "同一份配置问两次是同一个答案");
    }

    /// A data directory short enough for the socket to live inside it keeps it
    /// there: the launcher's own storage is where its own files belong, and the
    /// short path is the common case.
    #[test]
    fn a_short_data_directory_keeps_the_socket_inside_it() {
        let dir = Path::new("/Users/me/.dsh-launcher");
        let host = RemoteHost::of(&remote_host("zjq_aliyun"), dir).unwrap();

        assert!(
            host.socket.starts_with(dir.join("ssh")),
            "能放下就放在启动器数据目录下：{}",
            host.socket.display()
        );
    }

    /// Both of ssh's ways of saying "this path is too long" are about the
    /// launcher's own socket, not about the machine — and one of them says
    /// nothing at all about the limit. Neither may reach the user as raw ssh
    /// output with 「确认那台机器开着」 attached to it.
    #[test]
    fn sshs_two_too_long_messages_become_one_readable_reason() {
        let refused = ssh_failure(
            "lab",
            "ControlPath too long ('/Users/me/deep/dir/ssh/0123456789abcdef.sock' >= 104 bytes)\r\n",
        );
        let unbound = ssh_failure(
            "lab",
            "unix_listener: path \"/Users/me/deep/dir/ssh/0123456789abcdef.sock.kz3T78nCyigS9fZR\" too long for Unix domain socket\r\n",
        );

        for error in [&refused, &unbound] {
            assert!(
                error.message.contains("控制 socket"),
                "要说是启动器自己的 socket 太长：{}",
                error.message
            );
            assert!(
                error.message.contains("86"),
                "要把真正的上限说出来：{}",
                error.message
            );
            assert!(
                !error.message.contains("确认 ssh"),
                "这不是机器的问题，别提机器：{}",
                error.message
            );
        }
    }

    /// The real OpenSSH on the real path: it must be able to bind the socket the
    /// launcher derives from a data directory as long as the stock one, which is
    /// the case ssh refuses when the socket is kept inside it. It writes nothing
    /// into that machine's tree — the check is that ssh gets a session at all —
    /// and takes its socket away again afterwards.
    ///
    /// ```text
    /// DSH_LAUNCHER_TEST_SSH_HOST=zjq_aliyun \
    ///   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored a_real_ssh_binds
    /// ```
    #[tokio::test]
    #[ignore = "需要一台能免密登录的 ssh 主机（DSH_LAUNCHER_TEST_SSH_HOST）"]
    async fn a_real_ssh_binds_the_socket_the_launcher_derives() {
        let Ok(target) = std::env::var("DSH_LAUNCHER_TEST_SSH_HOST") else {
            eprintln!("跳过：没有设置 DSH_LAUNCHER_TEST_SSH_HOST");
            return;
        };
        let home = std::env::var("HOME").expect("这台机器上要有 HOME");
        let dir = PathBuf::from(home).join("Library/Application Support/in.dsh-plug.dsh-launcher");
        let host = RemoteHost::of(&remote_host(&target), &dir).unwrap();
        let transport = SshTransport::new();
        let transport: &dyn HostTransport = &transport;

        let out = transport
            .run(&host, RemoteCall::new("echo bound"))
            .await
            .unwrap_or_else(|e| panic!("ssh 连不上（socket {}）：{e}", host.socket.display()));

        assert!(out.stdout.contains("bound"), "{:?}", out.stdout);
        assert!(socket_len(&host) <= SOCKET_BUDGET, "{}", host.socket.display());
        // End the launcher's own connection first, the way the app's exit path
        // does, and only then take the socket away: a master that is still alive
        // owns it.
        let _ = transport.close_connection(&host).await;
        let _ = std::fs::remove_file(&host.socket);
        if let Some(parent) = host.socket.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    /// The argv has to be more than the right words: ssh has to **accept** it.
    /// `ssh -G` prints the configuration it derived and stops, so this needs no
    /// server — and it is what catches a `-o` value ssh cannot parse (a path
    /// with a space in it, unquoted, is refused outright).
    #[test]
    fn ssh_parses_the_argument_list_the_launcher_builds() {
        let dir = Path::new("/Users/me/Library/Application Support/in.dsh-plug.dsh-launcher");
        let host = RemoteHost::of(&remote_host("zjq_aliyun"), dir).unwrap();
        let argv = ssh_argv(&host.target, &host.socket, &script_command("echo hi"));

        let out = std::process::Command::new(SSH_BIN)
            .arg("-G")
            .args(&argv)
            .output()
            .expect("本机要有 /usr/bin/ssh");
        assert!(
            out.status.success(),
            "ssh 不接受这套参数：{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let config = String::from_utf8_lossy(&out.stdout);
        assert!(config.contains("batchmode yes"), "{config}");
        assert!(
            config
                .lines()
                .any(|line| line.trim() == format!("controlpath {}", host.socket.display())),
            "ssh 解析出来的 ControlPath 要正好是启动器自己的 socket：{config}"
        );
    }

    /// The script has to arrive at the remote shell as one word — ssh joins the
    /// command words and the remote shell splits them again, so nothing in this
    /// position may be left unquoted.
    #[test]
    fn the_script_reaches_the_remote_shell_as_one_word() {
        let script = "printf 'a b' \"$HOME\"";
        let command = script_command(script);
        assert_eq!(command[0], REMOTE_SHELL);
        assert_eq!(command[1], "-ilc");
        assert_eq!(command.len(), 3);
        assert_eq!(command[2], "'printf '\\''a b'\\'' \"$HOME\"'");

        // And one real shell must put it back together as the same script.
        let echoed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", crate::remote::quote(script)))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&echoed.stdout), script);
    }

    /// A forward's local end is bound to loopback **by spelling it out**:
    /// `port:host:port` alone is bound according to `GatewayPorts`, and a user's
    /// `~/.ssh/config` may turn that on — which would offer the instance's own
    /// address to this Mac's network. The remote end is carried verbatim, so an
    /// instance bound to that machine's LAN address is forwarded to that address.
    #[test]
    fn a_forward_spec_spells_out_the_loopback_bind() {
        let loopback = PortForward {
            local_port: 3080,
            remote_host: "127.0.0.1".into(),
            remote_port: 3080,
        };
        assert_eq!(forward_spec(&loopback), "127.0.0.1:3080:127.0.0.1:3080");

        let lan = PortForward {
            local_port: 41000,
            remote_host: "192.168.1.5".into(),
            remote_port: 4000,
        };
        assert_eq!(forward_spec(&lan), "127.0.0.1:41000:192.168.1.5:4000");
    }

    /// One `-O` control request rides the connection the launcher already has;
    /// adding a forward carries the `-L` spec, and ending the connection carries
    /// nothing. Both are requests to an established master, which is why a
    /// caller must not read them as "start a connection".
    #[test]
    fn control_requests_carry_the_forward_spec_only_when_they_need_one() {
        let forward = PortForward {
            local_port: 3080,
            remote_host: "127.0.0.1".into(),
            remote_port: 3080,
        };
        assert_eq!(
            control_command("forward", Some(&forward)),
            vec!["-O", "forward", "-L", "127.0.0.1:3080:127.0.0.1:3080"]
        );
        assert_eq!(
            control_command("cancel", Some(&forward)),
            vec!["-O", "cancel", "-L", "127.0.0.1:3080:127.0.0.1:3080"]
        );
        assert_eq!(control_command("exit", None), vec!["-O", "exit"]);
    }

    /// A free target may carry its own ssh arguments, and they belong before the
    /// remote command — after it, ssh would hand them to the remote shell.
    #[test]
    fn a_free_target_keeps_its_own_arguments() {
        let dir = Path::new("/tmp/launcher");
        let host = RemoteHost::of(&remote_host("user@example.com -p 2222"), dir).unwrap();
        let argv = ssh_argv(&host.target, &host.socket, &script_command("echo hi"));

        let target_at = argv.iter().position(|a| a == "user@example.com").unwrap();
        let shell_at = argv.iter().position(|a| a == REMOTE_SHELL).unwrap();
        assert_eq!(argv[target_at + 1], "-p");
        assert_eq!(argv[target_at + 2], "2222");
        assert!(
            target_at < shell_at,
            "目标自己的参数要排在远端命令之前：{argv:?}"
        );
    }

    /// A Unix socket path is length-limited, and two hosts must never share one.
    #[test]
    fn two_targets_never_share_a_socket_and_the_path_stays_short() {
        let dir = Path::new("/Users/someuser/Library/Application Support/in.dsh-plug.dsh-launcher");
        let a = RemoteHost::of(&remote_host("zjq_aliyun"), dir).unwrap();
        let b = RemoteHost::of(&remote_host("user@example.com -p 2222"), dir).unwrap();

        assert_ne!(a.socket, b.socket);
        assert!(
            a.socket.to_string_lossy().len() < 104,
            "{}",
            a.socket.display()
        );
        assert_eq!(a.socket.extension().unwrap(), "sock");
    }

    /// The local host is this Mac: there is nothing to dial and nothing to
    /// test, and saying so is better than a silent dial to an empty target.
    #[test]
    fn the_local_host_has_nothing_to_dial() {
        let err = RemoteHost::of(&Host::local(), Path::new("/tmp/launcher")).unwrap_err();
        assert!(err.message.contains("本机"), "拒绝的理由要说清是本机：{}", err.message);
    }

    /// The four classes, each with a message a user can act on. The first
    /// connection to a machine whose key is not in `known_hosts` fails under
    /// `BatchMode`; that failure has to point at the terminal, because the
    /// launcher can neither show the prompt nor take the password.
    #[test]
    fn ssh_failures_are_classified_into_four_readable_classes() {
        let refused = ssh_failure("lab", "ssh: connect to host lab port 22: Connection refused\n");
        assert_eq!(refused.kind, HostErrorKind::Unreachable);
        assert!(
            refused.message.contains("Connection refused"),
            "{}",
            refused.message
        );
        assert_eq!(refused.label(), "连不上");

        let denied = ssh_failure("lab", "user@lab: Permission denied (publickey).\n");
        assert_eq!(denied.kind, HostErrorKind::Auth);
        assert!(
            denied.message.contains("ssh lab") && denied.message.contains("终端"),
            "认证失败要指向「先在终端里连一次」：{}",
            denied.message
        );

        let unknown_key = ssh_failure("lab", "Host key verification failed.\n");
        assert_eq!(unknown_key.kind, HostErrorKind::Auth);
        assert!(
            unknown_key.message.contains("终端") && unknown_key.message.contains("主机密钥"),
            "未知主机密钥也要指向终端：{}",
            unknown_key.message
        );

        let slow = ssh_failure("lab", "ssh: connect to host lab port 22: Operation timed out\n");
        assert_eq!(slow.kind, HostErrorKind::Timeout);
        assert_eq!(slow.label(), "超时");

        // What a firewall or a proxy says: the machine answered nothing the
        // launcher can use, and it is not an authentication problem.
        let dropped = ssh_failure("lab", "Connection closed by 198.18.0.65 port 22\n");
        assert_eq!(dropped.kind, HostErrorKind::Unreachable);
        assert!(dropped.message.contains("Connection closed"), "{}", dropped.message);

        let failed = HostError::remote_command("远端命令失败（退出码 1）：npm ERR! missing pnpm");
        assert_eq!(failed.kind, HostErrorKind::RemoteCommand);
        assert!(failed.message.contains("npm ERR! missing pnpm"));
        assert_eq!(
            format!("{failed}"),
            "远端命令失败：远端命令失败（退出码 1）：npm ERR! missing pnpm"
        );
    }

    /// A login shell says `no job control in this shell` on every call; the
    /// failure reason must be what actually went wrong.
    #[test]
    fn the_stderr_tail_drops_the_login_shells_own_chatter() {
        let stderr = "bash: no job control in this shell\n\
                      bash: cannot set terminal process group (-1): Inappropriate ioctl for device\n\
                      sh: line 3: pnpm: command not found\n";
        let err = ssh_failure("lab", stderr);
        assert!(err.message.contains("pnpm: command not found"), "{}", err.message);
        assert!(!err.message.contains("no job control"), "{}", err.message);
    }

    /// The alias picker reads the user's config without re-implementing ssh's
    /// configuration language: aliases yes, catch-alls no, and an alias's own
    /// options are not its target.
    #[test]
    fn ssh_config_aliases_skip_wildcards_and_carry_the_target() {
        let text = "\
# the machine I care about
Host lab
    HostName 10.0.0.7
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_lab

Host *
    ServerAliveInterval 30

Host web db
    User=root

Host *.internal
    Port 22

Host after-wildcards
    HostName example.com

Match host lab
    Port 2200
";
        let aliases = parse_ssh_config(text);

        assert_eq!(
            aliases.iter().map(|a| a.alias.as_str()).collect::<Vec<_>>(),
            vec!["lab", "web", "db", "after-wildcards"],
            "通配的块不是别名；Match 块的关键字也不属于任何别名"
        );
        assert_eq!(aliases[0].host_name.as_deref(), Some("10.0.0.7"));
        assert_eq!(aliases[0].user.as_deref(), Some("deploy"));
        assert_eq!(aliases[0].port, Some(2222));
        assert_eq!(aliases[1].user.as_deref(), Some("root"), "Host web db 一行两个别名");
        assert_eq!(aliases[2].user.as_deref(), Some("root"));
        assert_eq!(aliases[3].host_name.as_deref(), Some("example.com"));
        assert_eq!(aliases[3].port, None, "Match 块里的 Port 不属于它");
    }

    /// A machine with no `~/.ssh/config` has nothing to pick — that is not an
    /// error worth showing anyone, and the user can still type a free target.
    #[test]
    fn a_missing_ssh_config_is_no_aliases_not_an_error() {
        let path = std::env::temp_dir().join(format!("dsh-ssh-config-{}", uuid::Uuid::new_v4()));
        assert!(ssh_aliases_at(&path).is_empty());
    }
}
