use crate::ssh::{PortForward, RemoteHost};
use crate::AppState;
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use tokio::sync::Mutex;

const LOOPBACK: &str = "127.0.0.1";

const LOWEST_BORROWABLE_PORT: u16 = 1024;

const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

const EXIT_BUDGET: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ForwardKey {
    host_id: String,
    remote_host: String,
    remote_port: u16,
}

#[derive(Clone, Debug)]
struct OpenForward {
    host: RemoteHost,
    forward: PortForward,
}

#[derive(Default)]
pub struct Forwards {
    open: Mutex<HashMap<ForwardKey, OpenForward>>,
}

pub(crate) async fn reachable_url(
    state: &AppState,
    host: &RemoteHost,
    remote_url: &str,
) -> Result<String, String> {
    let (remote_host, remote_port) = target_of(remote_url)?;
    let key = ForwardKey {
        host_id: host.id.clone(),
        remote_host: remote_host.clone(),
        remote_port,
    };

    let mut open = state.forwards.open.lock().await;
    if let Some(existing) = open.get(&key) {
        if port_answers(existing.forward.local_port) {
            return local_url(remote_url, existing.forward.local_port);
        }
        open.remove(&key);
    }

    let local_port = if remote_port < LOWEST_BORROWABLE_PORT || port_answers(remote_port) {
        free_port()?
    } else {
        remote_port
    };
    let forward = PortForward {
        local_port,
        remote_host,
        remote_port,
    };
    state
        .host_transport
        .open_forward(host, forward.clone())
        .await
        .map_err(|e| tunnel_failure(&host.target, remote_url, &e.to_string()))?;
    open.insert(
        key,
        OpenForward {
            host: host.clone(),
            forward,
        },
    );
    local_url(remote_url, local_port)
}

pub(crate) async fn withdraw(state: &AppState, host_id: &str, remote_url: &str) {
    let Ok((remote_host, remote_port)) = target_of(remote_url) else {
        return;
    };
    let key = ForwardKey {
        host_id: host_id.to_string(),
        remote_host,
        remote_port,
    };
    let Some(entry) = state.forwards.open.lock().await.remove(&key) else {
        return;
    };
    if let Err(e) = state
        .host_transport
        .close_forward(&entry.host, entry.forward)
        .await
    {
        crate::log_warn!("撤销到 {} 的端口转发失败：{e}", entry.host.target);
    }
}

pub(crate) async fn withdraw_host(state: &AppState, host_id: &str) {
    let open: Vec<OpenForward> = {
        let mut map = state.forwards.open.lock().await;
        let keys: Vec<ForwardKey> = map
            .keys()
            .filter(|key| key.host_id == host_id)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| map.remove(&key))
            .collect()
    };
    if open.is_empty() {
        return;
    }

    let mut hosts: Vec<RemoteHost> = Vec::new();
    for entry in open {
        if let Err(e) = state
            .host_transport
            .close_forward(&entry.host, entry.forward)
            .await
        {
            crate::log_warn!("撤销到 {} 的端口转发失败：{e}", entry.host.target);
        }
        if !hosts.iter().any(|host| host.socket == entry.host.socket) {
            hosts.push(entry.host);
        }
    }
    for host in hosts {
        if let Err(e) = state.host_transport.close_connection(&host).await {
            crate::log_warn!("关闭到 {} 的连接失败：{e}", host.target);
        }
    }
}

pub(crate) async fn close_all(state: &AppState) {
    let open: Vec<OpenForward> = {
        let mut map = state.forwards.open.lock().await;
        map.drain().map(|(_, entry)| entry).collect()
    };
    if open.is_empty() {
        return;
    }

    let mut hosts: Vec<(RemoteHost, Vec<PortForward>)> = Vec::new();
    for entry in open {
        match hosts
            .iter_mut()
            .find(|(host, _)| host.socket == entry.host.socket)
        {
            Some((_, forwards)) => forwards.push(entry.forward),
            None => hosts.push((entry.host, vec![entry.forward])),
        }
    }

    for (host, forwards) in hosts {
        for forward in forwards {
            if let Err(e) = state.host_transport.close_forward(&host, forward).await {
                crate::log_warn!("撤销到 {} 的端口转发失败：{e}", host.target);
            }
        }
        if let Err(e) = state.host_transport.close_connection(&host).await {
            crate::log_warn!("关闭到 {} 的连接失败：{e}", host.target);
        }
    }
}

pub(crate) fn close_all_blocking(state: &AppState) {
    if state.forwards.open.blocking_lock().is_empty() {
        return;
    }
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let _ = runtime
                .block_on(async { tokio::time::timeout(EXIT_BUDGET, close_all(state)).await });
        });
    });
}

fn target_of(remote_url: &str) -> Result<(String, u16), String> {
    let url = reqwest::Url::parse(remote_url.trim())
        .map_err(|e| format!("远端实例的地址无法解析（{remote_url}）：{e}"))?;
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("远端实例的地址里没有主机名（{remote_url}）"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| format!("远端实例的地址里没有端口（{remote_url}）"))?;
    Ok((host.to_string(), port))
}

fn local_url(remote_url: &str, local_port: u16) -> Result<String, String> {
    let mut url = reqwest::Url::parse(remote_url.trim())
        .map_err(|e| format!("远端实例的地址无法解析（{remote_url}）：{e}"))?;
    url.set_host(Some(LOOPBACK))
        .map_err(|e| format!("无法把远端实例的地址改到本机（{remote_url}）：{e}"))?;
    url.set_port(Some(local_port))
        .map_err(|_| format!("无法把远端实例的地址改到本机（{remote_url}）：{local_port}"))?;
    Ok(url.to_string())
}

fn port_answers(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), PROBE_TIMEOUT).is_ok()
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind((LOOPBACK, 0))
        .map_err(|e| format!("找不到可用的本机端口：{e}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("找不到可用的本机端口：{e}"))
}

fn tunnel_failure(target: &str, remote_url: &str, detail: &str) -> String {
    format!(
        "隧道没通（{detail}）：实例还在「{target}」上运行，它的地址是 {remote_url}。\
         这是本机到那台机器的连接断了，不是 DSH 拒绝了这次操作。\
         可以先在本机终端里执行 ssh {target} 确认能登录，再重试。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Host;
    use crate::remote::testing::{FakeAnswer, FakeCall, FakeTransport};
    use crate::ssh::{HostErrorKind, HostTransport};
    use std::path::Path;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TOKEN: &str = "JRFSsyvQL-n4ZNCC0i1GHUcAuQrlIntDazoDHwMhKsQ";
    const TARGET: &str = "zjq_aliyun";

    fn app_state(transport: Arc<dyn HostTransport>) -> AppState {
        AppState {
            config_path: Default::default(),
            data_dir: Default::default(),
            config: Default::default(),
            running: tokio::sync::Mutex::new(HashMap::new()),
            tasks: tokio::sync::Mutex::new(HashMap::new()),
            host_transport: transport,
            host_statuses: Default::default(),
            remote_scan: Default::default(),
            forwards: Default::default(),
        }
    }

    fn host(id: &str, target: &str) -> RemoteHost {
        RemoteHost::of(
            &Host {
                id: id.into(),
                display_name: "服务器".into(),
                ssh_target: target.into(),
                registry: None,
                extra_env: Default::default(),
                data_dir: Some(std::path::PathBuf::from(
                    "/home/u/.local/share/dsh-launcher",
                )),
                builtin: false,
                toolchain: Default::default(),
                toolchain_overrides: Default::default(),
            },
            Path::new("/launcher"),
        )
        .unwrap()
    }

    fn instance_url(port: u16) -> String {
        format!("http://127.0.0.1:{port}/?token={TOKEN}")
    }

    fn asked(fake: &FakeTransport) -> Vec<(RemoteHost, PortForward)> {
        fake.calls()
            .into_iter()
            .filter_map(|call| match call {
                FakeCall::OpenForward { host, forward } => Some((host, forward)),
                _ => None,
            })
            .collect()
    }

    fn withdrawn(fake: &FakeTransport) -> (Vec<PortForward>, Vec<RemoteHost>) {
        let mut forwards = Vec::new();
        let mut connections = Vec::new();
        for call in fake.calls() {
            match call {
                FakeCall::CloseForward { forward, .. } => forwards.push(forward),
                FakeCall::CloseConnection(host) => connections.push(host),
                _ => {}
            }
        }
        (forwards, connections)
    }

    fn free_local_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn a_forward_targets_the_run_records_address_and_binds_loopback() {
        let port = free_local_port();
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = app_state(fake.clone());
        let host = host("host-a", TARGET);

        let local = reachable_url(&state, &host, &instance_url(port))
            .await
            .unwrap();

        let open = asked(&fake);
        assert_eq!(open.len(), 1, "{open:?}");
        let (called_host, forward) = &open[0];
        assert_eq!(called_host.socket, host.socket, "走的是启动器自己那条连接");
        assert_eq!(
            forward.remote_host, "127.0.0.1",
            "转发目标照搬运行记录里的地址"
        );
        assert_eq!(forward.remote_port, port);
        assert_eq!(forward.local_port, port, "本机端口默认与远端相同");
        assert!(
            crate::ssh::forward_spec(forward).starts_with("127.0.0.1:"),
            "本地侧只绑回环：{}",
            crate::ssh::forward_spec(forward)
        );
        assert_eq!(local, instance_url(port));
    }

    #[tokio::test]
    async fn a_taken_local_port_moves_the_forward_to_a_free_one() {
        let port = free_local_port();
        let taken = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = app_state(fake.clone());

        let local = reachable_url(&state, &host("host-a", TARGET), &instance_url(port))
            .await
            .unwrap();

        let (_, forward) = asked(&fake).remove(0);
        assert_eq!(forward.remote_port, port, "远端那一头不动");
        assert_ne!(forward.local_port, port, "本机这个端口已经被占了");
        assert_eq!(local, instance_url(forward.local_port));
        assert!(
            TcpStream::connect_timeout(
                &SocketAddr::from(([127, 0, 0, 1], forward.local_port)),
                PROBE_TIMEOUT
            )
            .is_err(),
            "落到的端口是空闲的"
        );
        drop(taken);
    }

    #[tokio::test]
    async fn a_port_the_mac_cannot_bind_moves_to_a_free_one() {
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = app_state(fake.clone());

        let local = reachable_url(
            &state,
            &host("host-a", TARGET),
            "http://127.0.0.1:80/?token=x",
        )
        .await
        .unwrap();

        let (_, forward) = asked(&fake).remove(0);
        assert_eq!(forward.remote_port, 80, "远端那一头照搬");
        assert!(
            forward.local_port >= LOWEST_BORROWABLE_PORT,
            "本机端口不能要 root 才能绑：{}",
            forward.local_port
        );
        assert!(local.starts_with(&format!("http://127.0.0.1:{}", forward.local_port)));
    }

    #[tokio::test]
    async fn a_forward_is_reused_while_it_lives_and_rebuilt_once_it_is_gone() {
        let port = free_local_port();
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = app_state(fake.clone());
        let host = host("host-a", TARGET);

        let first = reachable_url(&state, &host, &instance_url(port))
            .await
            .unwrap();
        let alive = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let again = reachable_url(&state, &host, &instance_url(port))
            .await
            .unwrap();
        assert_eq!(again, first);
        assert_eq!(asked(&fake).len(), 1, "还活着的那条不用再问一次");

        drop(alive);
        fake.answering(FakeAnswer::Done);
        let rebuilt = reachable_url(&state, &host, &instance_url(port))
            .await
            .unwrap();
        assert_eq!(rebuilt, first);
        assert_eq!(asked(&fake).len(), 2, "端口没了就要重建");
    }

    #[tokio::test]
    async fn instances_on_one_host_share_one_connection() {
        let fake = FakeTransport::new();
        for _ in 0..3 {
            fake.answering(FakeAnswer::Done);
        }
        let state = app_state(fake.clone());
        let first = host("host-a", TARGET);
        let second = host("host-b", "other_aliyun");

        reachable_url(&state, &first, &instance_url(free_local_port()))
            .await
            .unwrap();
        reachable_url(&state, &first, &instance_url(free_local_port()))
            .await
            .unwrap();
        reachable_url(&state, &second, &instance_url(free_local_port()))
            .await
            .unwrap();

        let sockets: Vec<String> = asked(&fake)
            .into_iter()
            .map(|(host, _)| host.socket.display().to_string())
            .collect();
        assert_eq!(sockets.len(), 3);
        assert_eq!(sockets[0], sockets[1], "同一台主机的实例共用一条连接");
        assert_ne!(sockets[0], sockets[2], "另一台主机另一条连接");
    }

    #[tokio::test]
    async fn exiting_withdraws_every_forward_and_the_connections() {
        let fake = FakeTransport::new();
        for _ in 0..8 {
            fake.answering(FakeAnswer::Done);
        }
        let state = app_state(fake.clone());
        let first = host("host-a", TARGET);
        let second = host("host-b", "other_aliyun");
        reachable_url(&state, &first, &instance_url(free_local_port()))
            .await
            .unwrap();
        reachable_url(&state, &first, &instance_url(free_local_port()))
            .await
            .unwrap();
        reachable_url(&state, &second, &instance_url(free_local_port()))
            .await
            .unwrap();

        close_all(&state).await;

        let (forwards, connections) = withdrawn(&fake);
        assert_eq!(forwards.len(), 3, "每条转发都要撤掉");
        assert_eq!(connections.len(), 2, "每台主机只关一条连接");
        assert!(forwards
            .iter()
            .all(|f| crate::ssh::forward_spec(f).starts_with("127.0.0.1:")));

        fake.answering(FakeAnswer::Done);
        reachable_url(&state, &first, &instance_url(free_local_port()))
            .await
            .unwrap();
        assert_eq!(asked(&fake).len(), 4);
    }

    #[test]
    fn the_exit_path_withdraws_what_the_session_opened() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let fake = FakeTransport::new().answering(FakeAnswer::Done);
        let state = app_state(fake.clone());
        let host = host("host-a", TARGET);
        runtime
            .block_on(reachable_url(
                &state,
                &host,
                &instance_url(free_local_port()),
            ))
            .unwrap();
        fake.answering(FakeAnswer::Done).answering(FakeAnswer::Done);

        close_all_blocking(&state);

        let (forwards, connections) = withdrawn(&fake);
        assert_eq!(forwards.len(), 1, "每条转发都要撤掉");
        assert_eq!(connections.len(), 1, "连接也要关掉");
        assert!(state.forwards.open.blocking_lock().is_empty());
    }

    #[tokio::test]
    async fn a_broken_tunnel_says_so_and_offers_the_ssh_command() {
        let fake = FakeTransport::new().answering(FakeAnswer::fail(
            HostErrorKind::Unreachable,
            "ssh: connect to host zjq_aliyun port 22: Connection refused",
        ));
        let url = instance_url(free_local_port());

        let reason = reachable_url(&app_state(fake), &host("host-a", TARGET), &url)
            .await
            .unwrap_err();

        assert!(reason.contains("隧道"), "{reason}");
        assert!(reason.contains(&url), "远端原始地址要念出来：{reason}");
        assert!(
            reason.contains("ssh zjq_aliyun"),
            "给出可直接执行的命令：{reason}"
        );
        assert!(
            reason.contains("不是 DSH 拒绝"),
            "与「DSH 拒绝了这次操作」必须读起来不一样：{reason}"
        );
    }

    #[tokio::test]
    #[ignore = "需要一台能免密登录的 ssh 主机（DSH_LAUNCHER_TEST_SSH_HOST）"]
    async fn a_real_ssh_carries_the_address_over_the_forward() {
        let Ok(target) = std::env::var("DSH_LAUNCHER_TEST_SSH_HOST") else {
            eprintln!("跳过：没有设置 DSH_LAUNCHER_TEST_SSH_HOST");
            return;
        };
        let remote_url = std::env::var("DSH_LAUNCHER_TEST_SSH_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:22/".to_string());
        let far_log = std::env::var("DSH_LAUNCHER_TEST_SSH_LOG").ok();
        let socket_dir = std::path::PathBuf::from("/tmp").join(format!(
            "d9{}",
            uuid::Uuid::new_v4().simple().to_string().split_at(6).0
        ));
        let host = RemoteHost::of(
            &Host {
                id: "host-real".into(),
                display_name: "真机".into(),
                ssh_target: target.clone(),
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
        let state = app_state(Arc::new(crate::ssh::SshTransport::new()));

        let local = reachable_url(&state, &host, &remote_url).await.unwrap();

        let url = reqwest::Url::parse(&local).unwrap();
        let port = url.port().unwrap();
        match &far_log {
            None => {
                let mut socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut banner = [0u8; 256];
                let read = std::io::Read::read(&mut socket, &mut banner).unwrap();
                let banner = String::from_utf8_lossy(&banner[..read]).to_string();
                assert!(
                    banner.starts_with("SSH-"),
                    "本机端口 {port} 上回来的该是那台机器的 sshd，实际是：{banner}"
                );
            }
            Some(log) => {
                use std::io::Write;
                let mut request = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                request
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                write!(
                    request,
                    "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
                    match url.query() {
                        Some(query) => format!("{}?{query}", url.path()),
                        None => url.path().to_string(),
                    }
                )
                .unwrap();
                let mut reply = String::new();
                let _ = std::io::Read::read_to_string(&mut request, &mut reply);
                assert!(
                    reply.starts_with("HTTP/1."),
                    "本机端口 {port} 上要能做完一次请求，实际回来：{reply}"
                );

                let written = state.host_transport.read_file(&host, log).await.unwrap();
                let written = String::from_utf8_lossy(&written).to_string();
                let token = url
                    .query_pairs()
                    .find(|(k, _)| k == "token")
                    .map(|(_, v)| v.to_string())
                    .expect("地址里要带 token");
                assert!(
                    written.contains(&format!("token={token}")),
                    "那台机器收到的请求里要有原样的 token：{written}"
                );
            }
        }

        close_all(&state).await;
        for _ in 0..20 {
            if !port_answers(port) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!port_answers(port), "退出时本机端口要一起放掉");
        std::fs::remove_dir_all(&socket_dir).ok();
    }

    #[tokio::test]
    async fn a_dsh_refusal_does_not_read_like_a_broken_tunnel() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let serving = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer).await;
            let _ = socket
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
        });
        let refused = match crate::native::NativeClient::connect(&instance_url(port)).await {
            Ok(_) => panic!("HTTP 403 必须报错"),
            Err(e) => e,
        };
        serving.await.unwrap();

        let broken = reachable_url(
            &app_state(FakeTransport::new().answering(FakeAnswer::fail(
                HostErrorKind::Unreachable,
                "ssh: connect to host zjq_aliyun port 22: Connection refused",
            ))),
            &host("host-a", TARGET),
            &instance_url(free_local_port()),
        )
        .await
        .unwrap_err();

        assert!(
            refused.contains("token"),
            "DSH 的答案是关于 token 的：{refused}"
        );
        assert!(
            !refused.contains("隧道"),
            "DSH 的拒绝不是隧道的错：{refused}"
        );
        assert!(
            broken.contains("隧道") && broken.contains("ssh zjq_aliyun"),
            "{broken}"
        );
        assert!(
            !broken.contains("重启实例"),
            "隧道断了不该让人去重启实例：{broken}"
        );
    }
}
