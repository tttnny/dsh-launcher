# 本机以外的机器

管「一台机器」这一层：本机之外的主机怎么被问到、怎么被操作、哪些事只能在对面发生。术语见
`GLOSSARY.md`（主机、本机、远程主机、远端数据目录），推理见 ADR-0006 / ADR-0007 /
ADR-0008。实例这条记录见 `instances.md`，版本与插件见 `versions-and-homes.md`，页面上怎么
显示见 `frontend.md`。

## 本机之外，启动器不碰本机文件系统

`Host::local_paths_only` 是那条规则——远程主机的 DSH_HOME、版本目录都在那台机器上，拿本机
的 `fs` 去建或删只会命中这台 Mac 上同名的路径。要在远端建或删，就走传输层（`remote.rs`
里那些脚本）：建 DSH_HOME、DSH_HOME 的 Profile 读写、问出远端已有的 `~/.dsh` 再补一条
记录（记录 id 按主机分开：`config::user_dsh_home_id`），全部经那些脚本。同理
`cleanup_orphan_homes` 只判本机：「这个目录不存在」这句话，本机的 `exists()` 只能对这台
Mac 说。

## 远端数据目录在那台机器上 settle 一次

主机记录里记的是那台机器 settle 出来的**绝对**路径（`~` 只有它展开得了，`..` / 末尾 `/`
按文本归成一个），`Host::data_dir` 是取用它的唯一入口。记录里还不是绝对路径时它**报错**，
不照原样交给传输层——远端 shell 会把字面 `~` 当成它自己当前目录里的名字，返回一份看起来
像答案的空清单。这句原因要原样带到调用者：实例行、实例详情与插件页拿它当「主机不可达」的
说明显示；托盘只问它「那台机器现在能问吗」这一个是非。收窄成一句泛泛的失败就只剩一个点
下去还是同一句话的「重试」。

## 环境检测报的是「从哪来的」

每个工具的结论带来源——`system`（就来自系统环境变量）/ `fallback`（系统环境变量里没有，
启动器在常见安装目录里找到的）/ `manual`（用户选定）。「系统环境变量」= 用户登录 shell 的
`PATH`（另起一个登录 shell 读它，带超时与短时缓存；「重新检测」传 `force` 绕过缓存）；
远程那台一律经 `bash -ilc` 探测，因为只有交互式登录 shell 会读到 `~/.bashrc` 里配的
Linuxbrew。候选顺序固定为「用户指定 → 系统环境变量 → 常见安装目录」，**探测与执行共用这
一份**，别让页面报的与实际执行的不一致。**结论与用户选择都是按主机分开的**：探测缓存与
用户指定长在主机记录上（`Host.toolchain` / `Host.toolchain_overrides`），进程内的 `PATH`
缓存与远程探测缓存都按主机 id 分桶；`set_toolchain_override` 是唯一的写入通道
（`SettingsPatch` 不给这条路）。低于 20 的 Node 候选在远程**跳过并报出**
（`skipped_candidates`），本机维持「只告警不阻止」；远程缺工具时给的是**一条**带
`ssh <主机>` 前缀、可直接在本机终端执行的命令。**这套探测在 `src-tauri/src/runtime.rs`**
——`toolchain.rs` 只管 pnpm 策略（`MIN_PNPM_MAJOR`、registry、store），别照文件名去找
探测。

## ssh 控制 socket 有 86 字节的预算

所以 `ssh::socket_path` 能放进数据目录就放，放不下就退到 `$HOME/.dsh-launcher-ssh/`，名字里
带上数据目录与 ssh 目标两样东西的哈希——一个 socket 就是一条 ssh 连接，两份配置不能共用
一条。`$TMPDIR` 不行（`/var/folders/…` 一样长），`/tmp` 不行（别人可以先在那儿建
socket）。ssh 那两句原始报错（`ControlPath too long`、`too long for Unix domain socket`）
由 `ssh::ssh_failure` 换成一句能读的话——它说的不是那台机器，别把人引到「确认机器开着」
上去。同一条规矩也管「255 而 stderr 为空」的失败：那句话只说它知道的事（既可能是连不上，
也可能是启动器自己刚断开），并给出「先在终端里连一次、再重试」这个动作。

## 打开远端实例 = 先把那个地址挪到本机回环

`src-tauri/src/tunnel.rs` 是唯一建端口转发的地方——按主机合并，给每个运行中的实例加一条
`-L 127.0.0.1:<本地端口>:<运行记录里那个 URL 的 host:port>`，**本地侧一律只绑回环**
（`ssh::forward_spec` 显式写出 `127.0.0.1:`）。本机端口默认与远端相同；本机端口被占、或
低于 1024（用户态绑不了，实例可以把端口钉在 80）时改用一个空闲端口。**运行表里远端条目的
`url` 存的是「本机可达的地址」**（带 token），远端原始地址存在 `RemoteRun.remote_url`；
写这两样的地方只有两处（见 ADR-0008），**不要**把记录里那个远端原始地址直接填进 `url`
——那条路会让本机同端口的别的服务拿到 token，也让「打开」与托盘以为实例没就绪；而**远端
那份 `running.json` 仍然存远端原始地址**（它是转发目标）。**要地址就走
`process::reachable_url_for`**（「打开」、托盘与插件页都走它），它会把那条转发重新确认
一遍；退出时由 `tunnel::close_all_blocking` 撤掉启动器自己的转发与连接（用户自己的连接
在别的 socket 上，一个字节都不碰）。`InstanceStatus.url` 是本机可达的那个、`remote_url`
是远端位置，界面必须分开说。两种失败必须分开说：隧道没通给的是远端原始地址加一条可复制的
`ssh <主机>` 命令，并写明「不是 DSH 拒绝了这次操作」；「DSH 拒绝了」是实例 HTTP 层自己的
回答（`native.rs` 的 401/403 文案）。托盘的「打开」与双击也走同一条规则，失败用原生对话框
说原因。

## 远端实例在那台机器上起，也在那台机器上留名

`process::start_instance_process` 按实例的主机分叉，远端走
`HostTransport::spawn_detached`；环境取自 `launch::instance_env_for`，**不含本机代理**
——代理设置描述的是这台 Mac 的网络出口，远端出口由主机记录决定。启动前要问那台机器自己的
那几件事（`bin.js` 在不在、profile 是不是 web 应用、端口与 pnpm store）全部经传输层，
**不要**在本机 `fs` 上照远端路径读。远端实例的运行记录写在那台机器自己的 `running.json`
里，就是上面那条「启动 token 跨会话记住」在远端的落点。`kill_all`（退出启动器）跳过远端
实例：它们在别人机器上跑，本来就该活过这一次退出。

## 远端实例的停止与认领都发生在那一台主机上

`process::adopt_surviving_remote_instances` 按主机各起一遍，`settle_remote_instance` 是它
与页面「重试」共用的那一份判断。**发信号之前先核对身份**：读那台机器
`/proc/<pid>/stat` 的启动时间，与 `RemoteRun.started_at` 不符就**不发信号**（pid 被系统复
用了）；读不到时如实说读不到并给出在那台机器上停它的命令，绝不悄悄跳过。信号发给**进程
组**（`stop_group`），确认也用远端等（`remote::wait_gone`），只有「那台机器上看不到它了」
才算已停止。随机端口的 token 认证（`remote::mint_headers` + `native::cookie_pair_matches`）
必须**在那台机器上**做：远端实例绑的是那台机器的 `127.0.0.1`，从这台 Mac 问同一个 URL 会
问到另一个服务。

## 远端图标读一次要缓存

实例行每次渲染都会调 `read_instance_icon`，一行一次 ssh 是跑不动的。同一个会话里读过的
远端图标按实例 id 缓存在 `icons.rs` 的 `REMOTE_ICON_CACHE`（键里带上那条实例当前的 `icon`
值），设置或清除图标时丢掉对应的那一份。本机那一路不缓存。

## 有远端对应物的那几条命令

`open_home_directory` 与 `open_instance_directory` **继续保持本机专属**：它们的动作是
「在这台 Mac 的访达里打开那个目录」，对远程主机没有对应的意思，所以照旧经
`local_paths_only` 拒绝并把理由给出来，界面上也不给远程实例这个按钮。有远端对应物的是
另外几条，各自走传输层：`open_instance_terminal`（本机终端里一条 `ssh -t <目标> '<脚本>'`
连过去并进到 DSH_HOME，不写本机脚本文件）、`open_instance_log`（把那台机器上日志的最后
一段拉到 `<数据目录>/logs/<实例 id>.remote.log` 再打开，行为与本机一致）、
`instance_remote_location`（远端路径 + 一条可在本机终端执行的命令，供卡片上的「复制路径 /
复制命令」用）、以及图标三命令（图片在本机选、在本机裁，只有成品 PNG 经传输层写到那台机器
的 DSH_HOME 里）。
