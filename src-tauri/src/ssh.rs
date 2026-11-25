use crate::config::Host;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

pub const SSH_BIN: &str = "/usr/bin/ssh";

pub const REMOTE_SHELL: &str = "/bin/bash";

const CONNECT_TIMEOUT_SECS: u64 = 10;

const CONTROL_PERSIST_SECS: u64 = 60;

const SSH_FAILURE_STATUS: i32 = 255;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub type HostFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostErrorKind {
    Unreachable,
    Auth,
    Timeout,
    RemoteCommand,
}

impl HostErrorKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unreachable => "连不上",
            Self::Auth => "认证失败",
            Self::Timeout => "超时",
            Self::RemoteCommand => "远端命令失败",
        }
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shell {
    #[default]
    Plain,
    Login,
}

#[derive(Clone, Debug)]
pub struct RemoteCall {
    pub script: String,
    pub timeout: Duration,
    pub shell: Shell,
}

impl RemoteCall {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            timeout: DEFAULT_TIMEOUT,
            shell: Shell::Plain,
        }
    }

    pub fn with_login_shell(mut self) -> Self {
        self.shell = Shell::Login;
        self
    }

    pub fn within(script: impl Into<String>, timeout: Duration) -> Self {
        Self {
            script: script.into(),
            timeout,
            shell: Shell::Plain,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RemoteOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHost {
    pub id: String,
    pub target: String,
    pub socket: PathBuf,
}

impl RemoteHost {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortForward {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

pub fn forward_spec(forward: &PortForward) -> String {
    format!(
        "127.0.0.1:{}:{}:{}",
        forward.local_port, forward.remote_host, forward.remote_port
    )
}

fn control_command(action: &str, forward: Option<&PortForward>) -> Vec<String> {
    let mut argv = vec!["-O".to_string(), action.to_string()];
    if let Some(forward) = forward {
        argv.push("-L".to_string());
        argv.push(forward_spec(forward));
    }
    argv
}

pub trait HostTransport: Send + Sync {
    fn run<'a>(
        &'a self,
        host: &'a RemoteHost,
        call: RemoteCall,
    ) -> HostFuture<'a, Result<RemoteOutput, HostError>>;

    fn spawn_detached<'a>(
        &'a self,
        host: &'a RemoteHost,
        argv: &'a [String],
        log_path: &'a str,
        path_dirs: &'a [String],
    ) -> HostFuture<'a, Result<i32, HostError>>;

    fn stop_pid<'a>(
        &'a self,
        host: &'a RemoteHost,
        pid: i32,
    ) -> HostFuture<'a, Result<(), HostError>>;

    fn stop_group<'a>(
        &'a self,
        host: &'a RemoteHost,
        pid: i32,
    ) -> HostFuture<'a, Result<(), HostError>>;

    fn read_file<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
    ) -> HostFuture<'a, Result<Vec<u8>, HostError>>;

    fn write_file<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
        contents: Vec<u8>,
    ) -> HostFuture<'a, Result<(), HostError>>;

    fn list_dir<'a>(
        &'a self,
        host: &'a RemoteHost,
        path: &'a str,
    ) -> HostFuture<'a, Result<Vec<String>, HostError>>;

    fn open_forward<'a>(
        &'a self,
        host: &'a RemoteHost,
        forward: PortForward,
    ) -> HostFuture<'a, Result<(), HostError>>;

    fn close_forward<'a>(
        &'a self,
        host: &'a RemoteHost,
        forward: PortForward,
    ) -> HostFuture<'a, Result<(), HostError>>;

    fn close_connection<'a>(
        &'a self,
        host: &'a RemoteHost,
    ) -> HostFuture<'a, Result<(), HostError>>;
}

#[derive(Default)]
pub struct SshTransport;

impl SshTransport {
    pub fn new() -> Self {
        Self
    }

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
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
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

    async fn script(&self, host: &RemoteHost, call: RemoteCall) -> Result<RemoteOutput, HostError> {
        let command = script_command(&call.script, call.shell);
        self.dial(host, &command, call.timeout).await
    }

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

    async fn end_connection(&self, host: &RemoteHost) -> Result<(), HostError> {
        self.dial(host, &control_command("exit", None), CONTROL_TIMEOUT)
            .await
            .map(|_| ())
    }
}

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
        path_dirs: &'a [String],
    ) -> HostFuture<'a, Result<i32, HostError>> {
        Box::pin(async move {
            let call = RemoteCall::new(crate::remote::spawn_script(argv, log_path, path_dirs));
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

fn config_value(value: &str) -> String {
    const PLAIN: &str = "-_./~:@+,=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    if value.chars().all(|c| PLAIN.contains(c)) {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

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

pub fn script_command(script: &str, shell: Shell) -> Vec<String> {
    vec![
        REMOTE_SHELL.to_string(),
        match shell {
            Shell::Plain => "-c",
            Shell::Login => "-ilc",
        }
        .to_string(),
        crate::remote::quote(script),
    ]
}

fn socket_path(launcher_data_dir: &Path, target: &str) -> PathBuf {
    let digest = Sha256::digest(target.as_bytes());
    let short: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();

    let own = launcher_data_dir.join("ssh").join(format!("{short}.sock"));
    if own.as_os_str().len() <= MAX_SOCKET_PATH {
        return own;
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if home.as_os_str().is_empty() {
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

const SHORT_SOCKET_DIR: &str = ".dsh-launcher-ssh";

const MAX_SOCKET_PATH: usize = 86;

pub fn ssh_failure(target: &str, stderr: &str) -> HostError {
    if let Some(message) = socket_path_too_long(stderr) {
        return HostError::unreachable(message);
    }
    let kind = ssh_failure_kind(stderr);
    let tail = stderr_tail(stderr);
    let silent = tail.is_empty();
    let said = if silent {
        "ssh 没有说明原因".to_string()
    } else {
        tail
    };
    let message = match kind {
        HostErrorKind::Auth => format!(
            "{said}。启动器只支持免密连接（密钥或 ssh-agent）：请先在终端里跑一次 \
             ssh {target} 确认能直接登录，第一次连接要在那里接受主机密钥（密码与口令启动器不保存）"
        ),
        HostErrorKind::Timeout => format!(
            "{said}。连接 {target} 超过 {CONNECT_TIMEOUT_SECS} 秒没有回应：确认网络能到这台机器"
        ),
        HostErrorKind::Unreachable => {
            if silent {
                format!(
                    "ssh 什么都没说就退出了：这既可能是连不上 {target}，\
                     也可能是启动器自己那条连接刚被断开（撤销转发、退出启动器时会这样）。\
                     先在终端里跑一次 ssh {target} 确认能登录，再重试一次"
                )
            } else {
                format!("{said}。确认 ssh {target} 这台机器开着、地址没写错")
            }
        }
        HostErrorKind::RemoteCommand => said,
    };
    HostError::new(kind, message)
}

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshAlias {
    pub alias: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

pub fn ssh_aliases() -> Vec<SshAlias> {
    match std::env::var_os("HOME") {
        Some(home) => ssh_aliases_at(&PathBuf::from(home).join(".ssh").join("config")),
        None => Vec::new(),
    }
}

pub fn ssh_aliases_at(path: &Path) -> Vec<SshAlias> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_ssh_config(&text),
        Err(_) => Vec::new(),
    }
}

pub fn parse_ssh_config(text: &str) -> Vec<SshAlias> {
    let mut aliases: Vec<SshAlias> = Vec::new();
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
            _ => {}
        }
    }
    aliases
}

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

    #[test]
    fn ssh_argv_carries_batch_mode_and_the_launchers_own_socket() {
        let dir = Path::new("/Users/me/Library/Application Support/dsh");
        let host = RemoteHost::of(&remote_host("zjq_aliyun"), dir).unwrap();
        let argv = ssh_argv(
            &host.target,
            &host.socket,
            &script_command("echo hi", Shell::Plain),
        );

        assert!(
            argv.contains(&"BatchMode=yes".to_string()),
            "必须非交互：{argv:?}"
        );
        assert!(
            argv.contains(&format!("ControlPath=\"{}\"", host.socket.display())),
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
        assert!(
            socket_len(&host) <= SOCKET_BUDGET,
            "{}",
            host.socket.display()
        );
        let _ = transport.close_connection(&host).await;
        let _ = std::fs::remove_file(&host.socket);
        if let Some(parent) = host.socket.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    #[test]
    fn ssh_parses_the_argument_list_the_launcher_builds() {
        let dir = Path::new("/Users/me/Library/Application Support/in.dsh-plug.dsh-launcher");
        let host = RemoteHost::of(&remote_host("zjq_aliyun"), dir).unwrap();
        let argv = ssh_argv(
            &host.target,
            &host.socket,
            &script_command("echo hi", Shell::Plain),
        );

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

    #[test]
    fn the_script_reaches_the_remote_shell_as_one_word() {
        let script = "printf 'a b' \"$HOME\"";
        let command = script_command(script, Shell::Login);
        assert_eq!(command[0], REMOTE_SHELL);
        assert_eq!(command[1], "-ilc");
        assert_eq!(command.len(), 3);
        assert_eq!(command[2], "'printf '\\''a b'\\'' \"$HOME\"'");

        let echoed = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", crate::remote::quote(script)))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&echoed.stdout), script);
    }

    #[test]
    fn only_a_call_that_needs_the_environment_gets_the_login_shell() {
        assert_eq!(script_command("true", Shell::Plain)[1], "-c");
        assert_eq!(
            script_command("true", RemoteCall::new("true").shell)[1],
            "-c",
            "默认是轻的：不给登录环境"
        );
        assert_eq!(
            script_command("true", RemoteCall::new("true").with_login_shell().shell)[1],
            "-ilc",
            "要按名字找工具的那几条才起交互式登录 shell"
        );
    }

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

    #[test]
    fn a_free_target_keeps_its_own_arguments() {
        let dir = Path::new("/tmp/launcher");
        let host = RemoteHost::of(&remote_host("user@example.com -p 2222"), dir).unwrap();
        let argv = ssh_argv(
            &host.target,
            &host.socket,
            &script_command("echo hi", Shell::Plain),
        );

        let target_at = argv.iter().position(|a| a == "user@example.com").unwrap();
        let shell_at = argv.iter().position(|a| a == REMOTE_SHELL).unwrap();
        assert_eq!(argv[target_at + 1], "-p");
        assert_eq!(argv[target_at + 2], "2222");
        assert!(
            target_at < shell_at,
            "目标自己的参数要排在远端命令之前：{argv:?}"
        );
    }

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

    #[test]
    fn the_local_host_has_nothing_to_dial() {
        let err = RemoteHost::of(&Host::local(), Path::new("/tmp/launcher")).unwrap_err();
        assert!(
            err.message.contains("本机"),
            "拒绝的理由要说清是本机：{}",
            err.message
        );
    }

    #[test]
    fn ssh_failures_are_classified_into_four_readable_classes() {
        let refused = ssh_failure(
            "lab",
            "ssh: connect to host lab port 22: Connection refused\n",
        );
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

        let slow = ssh_failure(
            "lab",
            "ssh: connect to host lab port 22: Operation timed out\n",
        );
        assert_eq!(slow.kind, HostErrorKind::Timeout);
        assert_eq!(slow.label(), "超时");

        let dropped = ssh_failure("lab", "Connection closed by 198.18.0.65 port 22\n");
        assert_eq!(dropped.kind, HostErrorKind::Unreachable);
        assert!(
            dropped.message.contains("Connection closed"),
            "{}",
            dropped.message
        );

        let failed = HostError::remote_command("远端命令失败（退出码 1）：npm ERR! missing pnpm");
        assert_eq!(failed.kind, HostErrorKind::RemoteCommand);
        assert!(failed.message.contains("npm ERR! missing pnpm"));
        assert_eq!(
            format!("{failed}"),
            "远端命令失败：远端命令失败（退出码 1）：npm ERR! missing pnpm"
        );
    }

    #[test]
    fn a_silent_ssh_failure_names_both_possibilities_instead_of_the_machine_being_off() {
        let silent = ssh_failure("lab", "");
        assert_eq!(silent.kind, HostErrorKind::Unreachable);
        assert!(
            !silent.message.contains("这台机器开着"),
            "启动器不知道那台机器是关着的，别让它这么说：{}",
            silent.message
        );
        assert!(
            silent.message.contains("启动器自己那条连接"),
            "要说出启动器自己切断连接这个可能：{}",
            silent.message
        );
        assert!(
            silent.message.contains("重试"),
            "要给一个能离开这句话的动作：{}",
            silent.message
        );
        assert!(silent.message.contains("ssh lab"), "{}", silent.message);

        let refused = ssh_failure(
            "lab",
            "ssh: connect to host lab port 22: Connection refused\n",
        );
        assert!(
            refused.message.contains("确认 ssh lab") && refused.message.contains("机器开着"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn the_stderr_tail_drops_the_login_shells_own_chatter() {
        let stderr = "bash: no job control in this shell\n\
                      bash: cannot set terminal process group (-1): Inappropriate ioctl for device\n\
                      sh: line 3: pnpm: command not found\n";
        let err = ssh_failure("lab", stderr);
        assert!(
            err.message.contains("pnpm: command not found"),
            "{}",
            err.message
        );
        assert!(!err.message.contains("no job control"), "{}", err.message);
    }

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
        assert_eq!(
            aliases[1].user.as_deref(),
            Some("root"),
            "Host web db 一行两个别名"
        );
        assert_eq!(aliases[2].user.as_deref(), Some("root"));
        assert_eq!(aliases[3].host_name.as_deref(), Some("example.com"));
        assert_eq!(aliases[3].port, None, "Match 块里的 Port 不属于它");
    }

    #[test]
    fn a_missing_ssh_config_is_no_aliases_not_an_error() {
        let path = std::env::temp_dir().join(format!("dsh-ssh-config-{}", uuid::Uuid::new_v4()));
        assert!(ssh_aliases_at(&path).is_empty());
    }
}
