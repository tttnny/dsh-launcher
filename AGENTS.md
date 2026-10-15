# AGENTS.md — DSH Launcher

Tauri 2 + Vue 3 的 macOS Apple Silicon 桌面启动器：管理多版本 DSH 与多实例，
每个实例有独立的 `DSH_HOME`、Profile 和环境变量。运行数据在
`~/Library/Application Support/in.dsh-plug.dsh-launcher`（布局见 README「运行数据」）。

**打开实例 = 用系统浏览器打开实例 URL（契约，勿回退）**：经 `open that`，
与 `open_external` 同策略（先 trim 再只放行 http 与 https）。
`open_instance_window` 保留命令名和只传 id 的签名，后端从 running 表查 URL；
不存在任何实例 webview（回退史见 ADR-0002）。

**导航只有三个目的地：实例 / 资源库 / 设置（契约，见 ADR-0004）**：
`src/router/index.ts` 是唯一的路由表，`src/shortcuts.ts` 是快捷键与后端
`menu-navigate` 载荷的唯一映射。旧路径（`/homes`、`/profiles`、`/versions`、
`/tasks`）必须继续重定向；托盘与原生菜单的载荷指向 `/library?tab=…`。
`/plugins` **不在这份名单里**——它一直是插件页本身（只由运行中的实例进入），
别把它改成重定向。加第四项顶级导航前先读 ADR-0004。

**实例是一条绑定记录（契约，见 ADR-0003）**：`create_instance` 只绑定已安装的
版本，不建目录、不碰 DSH_HOME；`delete_instance` 只删记录（加上实例自己的图标
文件），版本与 DSH_HOME 一律留下。破坏性删除只属于「删除版本」。
「复制实例」是同一个命令加一个预填的弹窗，不要再加 `duplicate_instance`。
**实例名按主机唯一**：两台机器可以各有一个叫 `web` 的实例，判重只看同一台主机
（`create_instance`、`update_instance` 与装版本的 `start_create_instance_task` 三处
同一条规则）——名字只在自己那台机器上需要不重名，而行、托盘与插件链接都带上主机。

**删版本要连专属 DSH_HOME 的记录一起删（契约，见 ADR-0002）**：`remove_version` 用
同一条路径规则（`<数据目录>/homes/<版本名>`）删两样东西——目录与配置里的那条记录，
两者在同一个状态保存里完成。只删目录会让记录比它指向的目录活得久：DSH_HOME 列表继续
展示已删版本的 HOME，点「打开目录」还会把目录重新建出来，启动时的孤儿清理（判据是
「目录不存在」）从此再也清不掉它。删完版本前端要一并刷新 DSH_HOME 列表
（`store.refreshHomes()`），浏览器 mock 的 `remove_version` 也照同一条路径规则删。

**插件管理属于运行中的实例（契约）**：入口只有实例侧两处（详情页头部按钮、实例行上的
按钮），且只在实例 `running` 时出现；Profile 列表不放插件入口，只陈述「哪些实例默认用
它」。跳转带 `?instanceId=` 与 `?from=`（`instances` /
`instance-detail`，由 `usePluginLink.openPlugins` 写入），「返回实例」据此回到用户
离开的地方：从实例行走就是「实例页 + `?focus=<id>`」（那一行的卡片自动展开、滚动到
位并闪一下），从详情页走就是回详情页。别再把它钉死到某一个页面。

**插件页只管理补丁层（契约）**：插件页 = 补丁层（Bundle）列表，*条目（entry）不做
控制面*——`list_plugins` 只调 `listBundles`，每条层上报它带多少条目（`row_count` →
`BundleItem.rowCount`），前端不列条目标题、不给条目标开关，因此也没有搜索 / 全选 /
批量开关（条目是 200+ 量级，补丁层是 30 以内：控制面收在层上才对）。行内降噪规则
不变：`bundleHasOwnTitle` 比较的是「标题与包名是否不同」，不同才占第二行。

**插件页的写操作与 DSH 自带插件页同源（契约）**：开关 = 原生 `setBundleEnabled`，
卸载 = 原生 `removeBundle`——同一个方法、同一个权威（运行中的实例）、同一把文件锁，
文案也跟着 DSH 叫「卸载」（`removeBundle` 对 Profile 装的依赖真的会 `pnpm remove`，
只对 installation 自带的层才是纯取消选择；叫「摘掉」会让人以为依赖留下）。**不做**的
是原生页比我们多的那几件事：条目级开关（`setPluginEnabled`）、安装 / 更新
（`installBundle` + 进度 + 取消）、每个层自己注册的配置页、版本兼容性豁免
（`setVersionExemption`）。要加其中任何一项，先读 `packages/boot/plugin-manager`
的 `types.ts`：它们的返回形状与进度事件比开关复杂一个量级。

**插件页按「归属」分两组，状态不做分类（契约）**：列表只有「你添加的」（`removable`，
默认展开，行内可卸载）与「随 DSH 发布」（`removable:false`，默认折叠，只报个数）——
**不要**按 `enabled` 再切一个「未挂载」组：挂没挂载是状态，开关和状态点已经说了，而且
按状态分组的折叠会让刚被切掉的那一行从指针底下跑掉。两组的组头都是折叠开关并带计数，
所以折叠不等于隐藏。段标题报状态总数（「已挂载 N / M」）。写入返回
`restart-required` 时，页面顶部留一条「改动要重启才生效」的提示 + 重启按钮（实例
状态一变就撤掉），不要只靠一条 toast。

**未运行时给的是「文件投影」，不是实例的答案（契约）**：没有实例应答（没在跑 / 被
接管 / 连不上）时，页面用 `plugins::offline_bundles_in` 读 Profile 自己的 `package.json`
——它声明的层（`dsh.profile.bundles`，即优先级顺序）在先，装了的依赖在后，
`version` 从 `node_modules/<name>/package.json` 读。**这份投影读的是「那台机器上的」
文件**：本机走 `std::fs`，远端经 `remote::profile_packages` 一次 ssh 把
`package.json` 与各包版本一起读回来（一个包一次连接会让「没有实例时」比有实例时还慢），
两边共用同一份纯投影（`plugins::offline_bundles_from`）。机器本身连不上时**不读**：那
路径不在本机，本机的 `fs` 只会问到这台 Mac 上同名的地方，页面给的是
「主机不可达」的原因与「重试」，而不是一份假列表。这份投影**只读**（ADR-0001 一条都
不破），而且**故意比在线列表薄**：没有实时状态、可用性、卸载规则、条目数，`removable`
一律 false——所以它在界面上也长得不一样：平铺、不分组（分组依据是 `removable`，文件
证明不了），开关禁用，脚注说明它读自文件。别把它当成实例的列表，也别据此把层归到
「随 DSH 发布」那一组。

**写入时锁住整页（契约）**：实例是拿一把 profile 文件锁串行写盘的，第二个点击会排在
第一个后面、还可能回一个看不懂的拒绝。所以只要有写入在飞（`bundlePending` 非空），
页面上所有开关与卸载按钮一起禁用（`BundleRow.locked`），跟原生插件页的行为一致。

**启动 token 要跨会话记住（契约）**：实例就绪时打印的 URL（含 `token=`）写进
`<data_dir>/running.json`（`process::remember_url`），实例停止 / 退出时删掉。收养
外部进程前先用它验证（`native::NativeClient::connect` 成功才沿用，失败即丢弃）——
否则每次「启动器更新或退出重开、实例还在跑」都会白丢 token，插件页从此只剩一句
「由系统接管」的死路。因此「能不能管理插件」只由 `process::manageable_url`（URL 里
有没有 token）决定，**不要**再把 `adopted` 当拒绝理由：`adopted` 只表示「停止时按
pid 杀」。只读横幅必须带一个能离开该状态的按钮：没在跑给「启动」、有 token 在跑给「重启」、
**主机不可达给「重试」**（`refresh_instance_status`，与实例行同一条规则）——不可达绝不能给
「启动」，那正是把第二个 DSH 起到服务器上的那一下。

**启动时收养幸存进程（契约）**：`process::adopt_surviving_instances` 在 app 启动后
后台跑一遍——被替换 / 强杀留下的 DSH 进程会一直占着端口，不主动认领的话，界面会一直
说「已停止」（插件页退化成文件列表），直到用户碰巧点一次启动。认领依据：用户钉死的
端口只要有 DSH 样进程即可；随机端口必须先用 `running.json` 里的 token 认证成功才认，
否则那个端口可能是别的程序。这一遍只看本机：远端实例的端口在那台机器上，本机探它
只会问到这台 Mac（远端那份记录由它自己的认领路径读）。

**远端实例的停止与认领都发生在那一台主机上（契约，见 ADR-0007）**：
`process::adopt_surviving_remote_instances` 按主机各起一遍（`settle_remote_instance` 是
它与页面「重试」共用的那一份判断），本机那一遍不许碰远端实例——本机的端口探针只能问这台
Mac。**发信号之前先核对身份**：读那台机器 `/proc/<pid>/stat` 的启动时间，与
`RemoteRun.started_at` 不符就**不发信号**（pid 被系统复用了）；读不到时如实说读不到并给出
在那台机器上停它的命令，绝不悄悄跳过。信号发给**进程组**（`stop_group`），确认也用远端等
（`remote::wait_gone`，一次 ssh 顶一段等待），只有「那台机器上看不到它了」才算已停止。
随机端口的 token 认证（`remote::mint_headers` + `native::cookie_pair_matches`）必须**在那台
机器上**做：远端实例绑的是那台机器的 `127.0.0.1`，从这台 Mac 问同一个 URL 会问到另一个
服务。

**「主机不可达」是第五个状态，它不是「已停止」（契约）**：`InstanceState::Unreachable`
（TS 的 `'unreachable'`、`home.status.unreachable`、`.dl-status.is-unreachable`）表示
「启动器现在没法知道这台机器上这个实例的状态」。启动时那一遍扫描还没问到的远端主机记在
`AppState.remote_scan` 里，`process::list_statuses` 对它报 `starting`，**绝不报 `stopped`**：
「已停止」是让用户去点「启动」的那句话，而在服务器上起了第二个 DSH 正是 ADR-0007 存在的
理由。主机状态（`AppState.host_statuses`）一旦是 `unreachable`，它的实例（包括本会话里
已经认为在跑的那些）就报 `unreachable`，页面给的是「重试」（`api.refreshInstanceStatus`
→ `commands::refresh_instance_status` → `settle_remote_instance`），不是「启动」。
`delete_instance` / `remove_version` 先把停止的错误带出来再删记录：删掉记录就再也没法说出
服务器上还在跑的是什么了。

**远端实例在那台机器上起，也在那台机器上留名（契约，见 ADR-0007）**：
`process::start_instance_process` 按实例的主机分叉——本机仍是原来的子进程路径，远端
走 `HostTransport::spawn_detached`（`setsid`、stdin 接 `/dev/null`、输出追加到
`<那台主机的数据目录>/logs/<实例 id>.log`）。命令行取自 `launch::instance_argv`（绝对
Node 来自那台机器自己的绑定，`--host` 仍然不传，`--no-open` 问那台机器的 bundle），
环境取自 `launch::instance_env_for`：DSH_HOME、实例名、实例自己的变量，再加主机记录里
的额外环境变量，**不含本机代理**——代理设置描述的是这台 Mac 的网络出口，远端出口由主机
记录决定。启动前要问那台机器自己的几件事（`bin.js` 在不在、profile 是不是 web 应用、
profile 的 lan-bind 端口与 pnpm store）全部经传输层，**不要**在本机 `fs` 上照远端路径读。
远端实例的运行记录写在那台机器自己的 `running.json` 里（按实例 id 索引
`{url, token, profile, pid, started_at}`：`pid` 与 `/proc/<pid>/stat` 的启动时间是停止前
核对用的，`url` 里的 token 是插件页唯一的凭据），进程起来时先写 pid 与启动时间，日志里
出现 `dsh web: <url>` 时补上 url 与 token——这份记录就是上面那条「启动 token 跨会话记住」
在远端的落点，没有它，启动器一重启插件页就只剩死路。跟随远端日志的那个任务只做两件事：
发现 URL、发现进程没了（`remote::tail` 在远端等，不是一秒一次 ssh）。`kill_all`（退出
启动器）跳过远端实例：它们在别人机器上跑，本来就该活过这一次退出。

**一台机器上一个全新的 DSH_HOME 先在那儿跑一次 DSH（契约）**：`profiles/web` 是 DSH
第一次在一个 HOME 里运行时才建出来的。本机那份由安装任务在建实例时补
（`tasks::ensure_web_profile_template`），另一台机器由
`process::ensure_remote_web_profile_template` 在**那台机器上**做同一件事：起一次
`node bin.js --profile web --host 127.0.0.1 --port 0`、等它打出 URL、按 pid 核对后停掉、
把 `profiles/web` 复制成 `profiles/__temp__`、再清掉复制件里那次启动钉住的端口。两个
建实例的入口都要走它——安装任务（`do_create_instance`）与「新建实例」
（`commands::create_instance_in`）——少了这一步，那个 HOME 下一个 Profile 都选不出来，
「启动」点了不会动。**判据是「那个 HOME 下一个 Profile 都没有」，不是「这是启动器建的
HOME」**：已经有 Profile 的 HOME 一个字节都不碰，本机那条路径也不因此改变。

**启动器自己的 ssh 控制 socket 有 86 字节的预算（契约）**：ssh 不直接绑 `ControlPath`，
它先绑 `<ControlPath>.<16 个随机字符>` 再硬链接过去，104 字节的 Unix socket 地址里留给
启动器的因此只有 86——macOS 默认数据目录算出来是 89 字节，ssh 直接拒绝（本机 OpenSSH
实测：86 可以、87 就报 `too long for Unix domain socket`）。所以 `ssh::socket_path`
能放进数据目录就放，放不下就退到 `$HOME/.dsh-launcher-ssh/`，名字里带上数据目录与 ssh
目标两样东西的哈希：一个 socket 就是一条 ssh 连接，两份配置不能共用一条。`$TMPDIR` 不行
（`/var/folders/…` 一样长），`/tmp` 不行（别人可以先在那个位置上建 socket）。ssh 那两句
原始报错（`ControlPath too long`、`too long for Unix domain socket`）由 `ssh::ssh_failure`
换成一句能读的话——它说的不是那台机器，别把人引到「确认机器开着」上去。

**打开远端实例 = 先把那个地址挪到本机回环（契约，见 ADR-0008）**：`src-tauri/src/tunnel.rs`
是唯一建端口转发的地方——按主机合并，同一个 `ControlSocket` 上给每个运行中的实例加一条
`-L 127.0.0.1:<本地端口>:<运行记录里那个 URL 的 host:port>`，**本地侧一律只绑回环**
（`ssh::forward_spec` 显式写出 `127.0.0.1:`，不靠 `GatewayPorts`）。本机端口默认与远端相同
（界面上的地址就是那台机器上真实存在的地址，跨次稳定）；本机端口被占、或低于 1024（用户态
绑不了，实例可以把端口钉在 80）时改用一个空闲端口。**运行表里远端条目的 `url` 存的是「本机
可达的地址」**（带 token），远端原始地址存在 `RemoteRun.remote_url`：`manageable_url`、插件
客户端、托盘、实例行与「打开」读的都是前者，而**远端那份 `running.json` 仍然存远端原始地址**
（它是转发目标，认领要照它建转发）。写这两样东西的地方只有两处——实例就绪（日志里出现 URL）
的 `follow_remote_instance` 与认领的 `adopt_remote_process`——两处都走
`process::local_address_for`（转发建不起来时它给 `None`：代价是地址，不是实例），**不要**把
记录里那个远端原始地址直接填进 `url`，那条路会让本机同端口的别的服务拿到 token，也让托盘与
「打开」以为实例没就绪。`InstanceStatus.url` 是本机可达的那个、`remote_url` 是远端位置，
界面必须分开说，别让人以为 DSH 绑在本机。**要地址就走
`process::reachable_url_for`**（「打开」、托盘与插件页都走它）：它把那条转发重新确认一遍再返回地址，
本机实例则原样返回自己的 URL——插件客户端读到的因此就是本机可达的那个地址，协议一个字都不用改。转发在实例就绪（日志里出现 URL）时建，
退出时由 `tunnel::close_all_blocking` 撤销：每条转发 `-O cancel`，每台主机再 `-O exit` 结束
启动器自己那条连接——用户自己的连接在别的 socket 上，一个字节都不碰。`ssh -O forward` 是发给
master 的请求、不会自己建连接，所以 `SshTransport::open_forward` 先把连接带起来；而 master 被
`ControlPersist` 收走时转发跟着消失（Mac 睡一觉就够），所以记住的转发在用之前先探本机端口，
死了就当没有、重建一条。两种失败必须分开说：隧道没通给的是远端原始地址加一条可复制的
`ssh <主机>` 命令，并且写明「不是 DSH 拒绝了这次操作」；「DSH 拒绝了」是实例 HTTP 层自己的
回答（`native.rs` 的 401/403 文案），不要与隧道混成一句。没有页面可以报错的两个入口——托盘
的「打开」与托盘双击——也走同一条规则，失败用原生对话框说原因。

**环境检测报的是「从哪来的」（契约，见 ADR-0005）**：每个工具的结论带来源——`system`
（就来自系统环境变量）/ `fallback`（系统环境变量里没有，启动器在常见安装目录里找到的）/
`manual`（用户选定）。「系统环境变量」= 用户登录 shell 的 `PATH`（另起一个登录 shell
读它，带超时与短时缓存；「重新检测」传 `force` 绕过缓存，否则用户配好 `.zshrc` 也刷不
出来）。候选顺序固定为「用户指定 → 系统环境变量 → 常见安装目录」，探测与执行共用这一份，
别让页面报的与实际执行的不一致。**结论与用户选择都是按主机分开的**：探测缓存与用户指定长在
主机记录上（`Host.toolchain` / `Host.toolchain_overrides`），进程内的 PATH 缓存与远程探测
缓存都按主机 id 分桶（不是单个槽，否则两台机器互相污染）；`set_toolchain_override` 是唯一的
写入通道（`SettingsPatch` 不给这条路），指定的文件失效时退回自动探测但如实把它带回去给页面
提示。远程那台一律经 `bash -ilc` 探测（与它在自己终端里命中的是同一个 shell），执行用绝对
路径，候选表是 Linux 的那一套（Linuxbrew、nvm / fnm 版本目录、pnpm 官方脚本目录、
`~/.local/bin`、`/usr/local/bin`）；低于 20 的 Node 候选**跳过并报出**（本机维持「只告警不
阻止」），跳过了谁与原因由后端放进 `skipped_candidates` 交给页面。远程的「手动指定」只能填
路径（远端没有本机文件选择器），由那台机器真跑一次 `--version` 才认；远程缺工具时给的是
**一条**带 `ssh <主机>` 前缀、可直接在本机终端执行的命令。

**DSH 的展示文案是「字符串或语言映射」（契约）**：`meta.title` / `meta.description`
对官方插件是普通字符串，对本地化过的插件是 `{"en": …, "zh": …}`。凡是有可能拿到
翻译的字段一律走 `native::NativeText`（`pick` 按 UI 语言取：精确 tag → 基础语言 →
`en` → 任意），**不要**声明成 `String`——一个翻译过的标题就会让整张列表解析失败
（症状：插件页弹「解析插件列表失败: invalid type: map, expected a string」）。
投影给前端的 `BundleItem` 里只有解析后的单语字符串，mock 也按这个投影后的形状写。

**整个启动器只有一个「当前主机」（契约，见 ADR-0006）**：状态住在
`src/stores/launcher.ts` 的 `currentHostId`，资源库页头的选择器与实例页的主机筛选读的是
同一份——两处各持一个 ref 就是「同一件事说两遍」。资源库的三个标签页永远按它显示
（`store.versions` / `store.homes` 就是它的那两份，按主机缓存），**没有**「全部」这一档；
实例页的「全部」是 `store.allHosts` 开的一个视图，落到「这台主机」时用的还是
`currentHostId`。所以：加主机选择控件时只改 store 里这一个状态，别再建第二个。

**实例页的批量按「看得见的那一份」来（契约）**：「全部启动 / 全部停止」作用于当前可见
的行（主机筛选 × 搜索框），规则只有一份，住在 `src/instancesBulk.ts` 的 `bulkScope`
（视图把 `filtered` 交给它，批量按钮、汇总计数都读它的结果），所以筛选、汇总与两个按钮
不会各说各的。**主机不可达的行不进「全部启动」**：那种状态下启动器说不出那台机器上是不是
已经在跑，这一行自己的动作是「重试」，而在服务器上起第二个 DSH 正是 ADR-0007 存在的理由
——跳过要报出来，不能悄悄少做。可见集里没有可启动的行时不显示「全部启动」（按钮不能承诺
一件不会做的事）。两个批量动作做完都按实际结果报数：成功几个、失败几个（附后端那句原因，
不要被 `allSettled` 吞掉）、跳过几个不可达。

**托盘是一份跨主机的清单（契约，见 ADR-0006 §4）**：托盘「运行中」子菜单列出**所有主机**
上运行中的实例，每行都带它跑在哪台机器上（`打开：名字（主机 · Profile）`）——两台机器上的
同名实例必须分得开。同一份清单决定双击：只有一个运行实例时直达它，两台机器各有一个**不算**
「只有一个」，显示主窗口（`sole_running`）。托盘的「打开」走 `process::reachable_url_for`
（与主窗口同一条地址规则，远端经隧道），「停止」走 `process::stop_instance_process`
（与主窗口同一套停止逻辑：pid 被复用不发信号、主机不可达如实说、进程不肯走如实说）。
托盘没有页面可以报错，失败经 `tray::report_to_user` 用原生对话框说；「停止全部」把失败
合成一份清单，不弹一串对话框。托盘的「全部启动 / 全部停止」**没有主机这一维**（ADR-0004/
0006），它的「全部」就是所有机器；但「全部启动」与页面同一条规则地**跳过不可达主机上的
实例**并把跳过的与失败的一起报出来（`bulk_start_targets` + `start_all_report`）——
那种状态下启动器说不出那台机器上是不是已经在跑。

**远端数据目录在那台机器上 settle 一次（契约，见 ADR-0006）**：主机记录里记的是那台机器
settle 出来的**绝对**路径（`~` 只有它展开得了，`..` / 末尾 `/` 按文本归成一个），
`Host::data_dir` 是取用它的唯一入口。记录里还不是绝对路径时它**报错**，不照原样交给传输层
——远端 shell 会把字面 `~` 当成它自己当前目录里的名字，返回一份看起来像答案的空清单。
这句原因要原样带到调用者：实例行、实例详情与插件页拿它当「主机不可达」的说明显示；托盘只问
它「那台机器现在能问吗」这一个是非——不可达主机上的实例不进「全部启动」，跳过的那一份由托盘
自己说。收窄成一句泛泛的失败就只剩一个点下去还是同一句话的「重试」。

**本机之外的主机，启动器不碰本机文件系统（契约）**：`Host::local_paths_only` 是那条规则
——远程主机的 DSH_HOME、版本目录都在那台机器上，拿本机的 `fs` 去建或删只会命中这台 Mac
上同名的路径。要在远端建或删，就走传输层（`remote.rs` 里那些脚本），别在本机上照
路径操作：建 DSH_HOME 就是这么做的——路径由那台机器展开 `~`、按文本
settle `..`、把目录建出来并确认可写，拿回来的绝对路径才写进记录
（`remote::ensure_home_path`，一句话一份脚本）；远端 DSH_HOME 的 Profile 读写也全部经
那些脚本。远端已有的 `~/.dsh` 由 `list_homes` 用一次 ssh 问出来再补一条记录（记录 id
按主机分开：`config::user_dsh_home_id`）——本机那条在 `load_config` 里由
`ensure_user_dsh_home` 补，它展开的是本机 `$HOME`。这条问询是尽力而为：那台机器没应答时
DSH_HOME 记录照常返回，因为记录是启动器自己的，随主机报错的是版本清单。同理
`cleanup_orphan_homes` 只判本机：「这个目录不存在」这句话，本机的 `exists()` 只能对这台 Mac
说，所以远程记录不会被它清掉。

`open_home_directory` 与 `open_instance_directory` **继续保持本机专属**：它们的动作是
「在这台 Mac 的访达里打开那个目录」，对远程主机没有对应的意思，所以照旧
经 `local_paths_only` 拒绝并把理由给出来，界面上也不给远程实例这个按钮。有远端对应物的是
另外几条，各自走传输层：`open_instance_terminal`（本机终端里一条 `ssh -t <目标> '<脚本>'`
连过去并进到 DSH_HOME，不写本机脚本文件）、`open_instance_log`（把那台机器上
`<远端数据目录>/logs/<实例 id>.log` 的最后一段拉到 `<数据目录>/logs/<实例 id>.remote.log`
再打开，行为与本机一致）、`instance_remote_location`（远端路径 + 一条可在本机终端执行的
命令，供卡片上的「复制路径 / 复制命令」用）、以及图标三命令（图片在本机选、在本机裁，
只有成品 PNG 经传输层写到那台机器的 DSH_HOME 里）。

**远端图标读一次要缓存（契约）**：实例行每次渲染都会调 `read_instance_icon`，一行一次 ssh
是跑不动的。同一个会话里读过的远端图标按实例 id 缓存在 `icons.rs` 的 `REMOTE_ICON_CACHE`
（键里带上那条实例当前的 `icon` 值），设置或清除图标时丢掉对应的那一份——命中给的是与
那台机器相同的答案，不是过期答案。本机那一路不缓存，也不经传输层。

**装版本与删版本都发生在那一台主机上（契约）**：`start_install_version_task` 带主机，
安装用那台主机的数据目录、registry、额外环境变量与探到的**绝对**工具路径，脚本里不出现
本机路径；进度与日志读的是那台机器上的日志文件，等新输出的那一秒是在远端度过的（一次
ssh 顶一段输出，不是一秒一次），脱离会话的命令没有 `wait` 可用，退出码由包装脚本写进同
一份日志。取消要真的停掉那边的**进程组**——`stop_pid` 只停组长，会在服务器上留下占着
store 锁的子进程，所以用 `stop_group`，而且要等那台机器确认进程真的没了才算「已取消」。
点「取消」与跑安装的 worker 会互相错过（点下去的那一刻正好没有命令在跑），所以已取消的
任务不再起新命令，worker 自己发现取消时也要停掉它刚起的那一个——两边都停，才不会留下
占着 store 的孤儿。
删版本时远端目录与专属 DSH_HOME 都由那台机器删，且只删那台主机数据目录之内的路径。
前端 `store.installingVersions` 按「主机 + 版本」索引：两台机器可以同时装同一个版本。

**实例行是控制面，展开卡片只是事实（契约）**：行本身讲的是「这台实例是什么、在不在跑」
——名字、版本 / DSH_HOME、运行时的 URL、选 Profile、状态、主操作，行尾一个只作指示的
展开箭头（点行或点实例名都展开，不是跳转）。运行或启动中的实例，进程态动作（重启 /
停止 / 管理插件）也长在行上：它们排成控件区的第二行，在主操作正下方（`.inst-tail`
竖排 + `.inst-tail-line` 横排、右对齐），这样「打开 / 重启 / 停止」都是一次点击，而不是
「先展开再点」——折叠是给读的，不是给按的。想再把它们收回卡片之前先读这一条：这是
用户明确嫌过的那次多余点击。`.inst-meta` 一行里，只有当实例属于**别的**主机、而且界面
同时在看多台机器（实例页的「全部」）时才多一个主机 chip：两台机器上的同名实例要分得开，
滤到某一台时不重复念同一个名字。

展开卡片里剩下的动作按「作用对象」分两段、各成一行：机器（终端 / 目录 / 日志）、
绑定（实例设置 / 复制实例 / 重新读取 Profile）。每段各自成行、段内才换行
（`.facts-actions` 竖排 + `.facts-line` 横排）：都挤在一行时尾部的按钮会被宽度顶成
孤立的一行（实例设置、复制实例都中过招）。不要再引入 `⋯` 之类的 hover 菜单，也不要
往行上塞图标按钮。机器那一段里，属于**别的**主机时「打开目录」换成「复制路径」与
「复制命令」两个按钮（仍然在这两段之内，段内换行）：那台机器上的 DSH_HOME 打不开，
能给的是路径与一条命令；终端与日志两个按钮不变，只是做的事在对面。

**不给 dsh 传 Node 运行时 flag（契约）**：`--preserve-symlinks` 已删除。它让 Node
把同一个包经多条路径加载成多个模块实例，而 DSH 的模块级状态按身份认人（工具调度器是
模块级 `Symbol`，`dsh-app-boot` 是模块级 WeakMap），于是整个进程坏掉：所有工具调用
报 `Cannot read properties of undefined (reading 'prepare')`，设置写入也全被拒。
代价是 `link:` 插件要靠自己 checkout 里的 `node_modules`（或 DSH 自己的内核包路由）。
要重新引入任何 node 运行时 flag，先确认 DSH 扛得住重复的模块实例。

**颜色只表示状态**：`src/styles/tokens.scss` 是唯一的调色板来源，机器事实
（版本号、端口、URL、路径、条目 id）一律加 `.dl-mono`。不要在视图里再写一份
按钮/卡片样式——`.dl-btn`、`.dl-surface`、`.dl-chip`、`.dl-status` 在
`src/styles/index.scss` 里定义一次（搜索框是 `.dl-input.dl-search`，实例列表与版本
列表共用）。

## 日常开发

- 验证走 `pnpm tauri dev`（真桌面 + Rust 后端）；`pnpm dev` 只有浏览器
  localStorage mock。
- 新增 Tauri 命令要同时改三处，否则 `ci/api-contract.test.mjs` 会失败：
  `src/api/index.ts`（interface）、`src/api/mock.ts`（switch case）、
  `src-tauri/src/lib.rs`（`generate_handler!`）。
- Rust 手工排版：全量 `cargo fmt` 会与并行 worktree 冲突，CI 的 `fmt --check` 不阻塞。

## 本机自测 App

自测副本固定在 `/Applications/dsh-launcher.app`。`pnpm tauri build --bundles app`
只产出 target 里的 `.app`；`pnpm app:local` 一步构建 + 签名 + 替换 + 校验
（`--no-build` 只补签 + 替换；会先退出运行中的实例）。

**签名必须用证书，ad-hoc 不行**：ad-hoc 的 designated requirement（DR）就是
`cdhash`，App 一重建，macOS 存的 TCC 授权（如「完全磁盘访问」）就失配——症状
隐蔽，系统设置里开关还开着、实际没权限。证书 DR 只钉 bundle id 和证书，重建后
不变；完整说明见 `ci/build-local-app.sh` 头注释，脚本发现 DR 含 cdhash 会直接报错退出。

- bundle id（`in.dsh-plug.dsh-launcher`）和安装路径别改，改了会重置授权。
- 证书约一年到期，DR 钉的是证书 CN（不是 Team ID），换发新证书要重新授权一次。

## 发版

推 tag 即发版（`git tag v<version> && git push origin v<version>`，tag 必须等于
`package.json` 的 version）。推 main 发预发布 `v<版本>-dev.<工作流序号>`，不动
Latest，也不抬版本号；`bump-version` job 只在**推 tag 且是正式版**时跑（工作流里的
条件就是 `startsWith(refs/tags/v) && prerelease == false`），它把 manifest 抬到下一个
patch 并推回 main。

- Release notes 手写（中英双语）；流水线只在正文还是占位符时填「下载表 + 提交
  列表」，改 `ci/release-notes-*.mjs` 后跑 `pnpm test:release-notes`。
- 发版 dmg 未签名未公证（只有本机自测副本走证书签名）；要改需 Developer ID
  证书（付费）并配到 CI。
- CI 挂掉时手工补发：`pnpm tauri build --bundles dmg`，再
  `gh release create v<版本> <dmg路径> --title v<版本> --notes-file <notes>`。

推送前本地复现质量门禁：

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace --locked
node ci/check-versions.mjs
```

## Agent skills

### Issue tracker

issue 与 spec 都是 GitHub issue（`tttnny/dsh-launcher`），用 `gh` CLI 操作 →
`docs/agents/issue-tracker.md`

### Triage labels

triage 的五个角色用中文标签串（`需要分诊`、`需要补充信息`、`待Agent处理`、
`待人工处理`、`不处理`）→ `docs/agents/triage-labels.md`

### Domain docs

单 context：根 `GLOSSARY.md` 词汇表 + `docs/adr/` 系统级 ADR →
`docs/agents/domain.md`
