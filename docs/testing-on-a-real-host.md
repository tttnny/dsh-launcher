# 在一台真机上验证

`pnpm verify` 跑的是本机的单元测试与源码级检查，它们盖不住「启动器跟一台真正的 Linux
机器连起来做一件事」这一整条路。这台 Mac 旁边就有一台可以随时用的服务器，而验证走一遍
真机是**决定性**的：有一次十步的验收行走抓出五个缺陷，305 个全绿的单元测试一个都没抓到
——其中一个会让真实用户的每一次 ssh 都失败。所以「没有可连的主机」不是结论。

读这份文档的两种情形：要写或跑 `#[ignore]`d 的真机测试；要在一台真实主机上端到端走一遍
（远端安装、起实例、隧道、插件页）。

## 那台机器

`zjq_aliyun` 就是为此准备的：从这台 Mac 免密可达（`ssh zjq_aliyun true` 直接成功，
加 `BatchMode=yes` 也一样），所以不用问任何人要凭据。

```
$ ssh zjq_aliyun 'uname -srm'
Linux 6.8.0-63-generic x86_64
$ ssh zjq_aliyun 'echo $HOME'
/home/zhou
```

它是 2 核 / 1.6 GB 的小机器，做原生编译时负载很高：一次远端安装实测 25–85 秒，走路时
`DEFAULT_TIMEOUT = 30s` 的调用预算抓到过超时（见 `docs/adr/0006` 里远端安装那一条）。
按这个量级定超时，别按本机的速度想。

### 用哪个 shell 问它，答案不一样（这就是 ADR-0005 追加那一条的理由）

| 问法 | node | pnpm |
| --- | --- | --- |
| `ssh zjq_aliyun 'cmd'`（非交互） | `/usr/bin/node` v18.19.1 | 找不到 |
| `ssh zjq_aliyun 'bash -lc "…"'`（登录 shell） | `/usr/bin/node` v18.19.1 | 找不到 |
| `ssh zjq_aliyun 'bash -ilc "…"'`（交互式登录 shell） | `/home/linuxbrew/.linuxbrew/bin/node` **v26.4.0** | `/home/linuxbrew/.linuxbrew/bin/pnpm` **12.11.2** |

Linuxbrew 的 `PATH` 配在只有交互式 shell 才读的 rc 里，所以启动器探测远端环境一律用
`bash -ilc`（ADR-0005）。**用 `-lc` 或普通 ssh 得出的「这台机器没有 pnpm」是错的**：
那样的结论会顺手给用户一条没必要的安装命令，而机器上明明有。

那台机器的 pnpm 是 12.x，启动器的 `MIN_PNPM_MAJOR = 11` 接受它，但 12 对「批准构建脚本」
的写法与 11 不同（`onlyBuiltDependencies` 在 12 上不生效，要显式包名）。在这台机器上撞到
`ERR_PNPM_IGNORED_BUILDS` 时先想到这一点。

## 跑那些 `#[ignore]`d 的真机测试

仓库现有三处，都是「没有真机就跳过」的形状：`DSH_LAUNCHER_TEST_SSH_HOST` 没设就打印一行
跳过并返回。照着它们的写法写新的，别另创一套。

```bash
DSH_LAUNCHER_TEST_SSH_HOST=zjq_aliyun \
  cargo test --manifest-path src-tauri/Cargo.toml -- --ignored
```

只想跑一个，就把 `--ignored` 后面接上名字过滤（`a_real_ssh`、`a_real_profile`、
`a_real_fresh_home`）。三处分别在：

- `src-tauri/src/tunnel.rs` 的 `a_real_ssh_carries_the_address_over_the_forward`——真的
  ssh 接受启动器拼出来的转发 argv、流量真的到对面、地址里的 token 原样抵达。可以用
  `DSH_LAUNCHER_TEST_SSH_URL` / `DSH_LAUNCHER_TEST_SSH_LOG` 指到一个会记日志的服务上。
- `src-tauri/src/remote.rs` 的 `a_real_profile_read_answers_from_that_machine`——对面
  自己的 shell 真的读懂一次 profile 读取脚本并一次答回来。
- `src-tauri/src/commands.rs` 的 `a_real_fresh_remote_home_comes_back_with_a_profile`
  ——远端一个全新的 DSH_HOME 真的能长出 `profiles/web`。这一条还要那台机器上装着一个版本
  （版本号可用 `DSH_LAUNCHER_TEST_REMOTE_VERSION` 指定，默认 `0.2.0-rc.2`）。

每条都自己清理它建的东西；新写的也要照这个规矩来。

## 无窗口 app 的 harness：直接驱动真正的命令函数

端到端走一遍界面背后的动作，不必启动桌面应用。做法是给 `AppState` 找一本临时的账：

```rust
let app = tauri::Builder::<tauri::Wry>::default()
    .build(tauri::test::mock_context(tauri::test::noop_assets()))
    .expect("构建无窗口的 app 失败");
app.manage(AppState { config_path, data_dir, /* … 真的 SshTransport … */ });
```

- 数据目录写在仓库的 `./tmp` 下，`config.json` 也是新的一份，所以**用户自己的启动器配置
  与实例一个字节都不碰**。
- `app.state::<AppState>()` 给出的就是 `#[tauri::command]` 要的 `State<'_, AppState>`，
  于是可以照着界面点按钮的顺序直接调 `commands::create_host(…)`、
  `tasks::start_install_version_task(…)`、`process::…`——走的是同一条代码路径，只有
  IPC 那一层没参与。
- `host_transport` 用真的 `SshTransport`：远端这几步的价值全在真连接上，换成假传输层就
  白走了。
- 异步部分跑在一个 `tokio` 多线程 runtime 上，收尾照退出路径走一遍
  `tunnel::close_all_blocking(&app.state::<AppState>())`，这样「退出启动器」这一步也真的
  被验到。

**坑**：macOS 上 Tauri/`tao` 的事件循环必须在主线程建，`cargo test` 里会直接炸在
`EventLoop must be created on the main thread!`。绕法是把它从 `main.rs` 里跑起来，用一个
环境变量开关（那次用的是 `DSH_ACCEPTANCE_WALK=1`），**走完即删**——不要把它留在仓库里。
一次完整的十步行走（连接 → 环境检测 → 装版本 → 建 DSH_HOME → 起实例 → 打开 → 插件页 →
退出重开认领 → 停止 → 清理）就是这个形状，副本在
`/Users/tny/Desktop/work/dsh-launcher-notes/spec9/acceptance-walk2-harness.rs`（不在仓库内）。

一个好用的小手法：临时数据目录故意取得比产品默认的更长（走路时用了 68 字节对 63 字节），
这样 ssh 控制 socket 会真的撞上 86 字节预算、走到退路那一条分支。

## 在那台机器上的规矩

- **自己建的东西全部删掉**：版本、DSH_HOME、pnpm store、日志、临时目录、ssh socket。
  收尾后回那台机器上确认（`ls` 一遍你动过的位置），并在报告里写明建了什么、删了什么。
- **那台机器上原本跑着的东西一律不碰**：`/home/zhou` 是用户的家目录，`~/.dsh` 可能是他
  真实的 DSH 数据，机器上已有的服务继续跑着。
- **`pnpm tauri dev` 不要对着用户的真实配置跑**：它会读写
  `~/Library/Application Support/in.dsh-plug.dsh-launcher/`，还会对用户真实实例动手。
  真机验证一律用上面那套无窗口的 harness + 临时数据目录；手工点击验证是用户自己的事。
- 只读地看一眼那台机器（`uname`、`echo $HOME`、`bash -ilc 'command -v pnpm'`）随时可以，
  上面的表就是这么量出来的。

## 什么时候值得走一遍

改到远端行为时（ssh 传输、远端安装、远端 DSH_HOME 与 Profile、远端起停与认领、隧道）都
值得——这批逻辑的失效方式是「本机看起来一切正常」。只改前端渲染、文案、样式时不必。
