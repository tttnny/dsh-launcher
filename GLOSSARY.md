# DSH Launcher

多版本、多实例的 DSH 桌面启动器。它管 DSH 的安装与运行，DSH 管 profile 里
装了什么插件——两个「管理」不是一回事，这个文件把它们分开。

## 语言

**DSH**：
DeepSeek Harness，被启动器安装和拉起的命令行工具。它的插件管理是它自己进程内的
一个服务，启动器是它的外部用户，不是它的上层。
_Avoid_: 应用、harness

**版本 (Version)**：
一份安装到独立目录的 DSH。同一台机器上并存多份，各自独立升级。
_Avoid_: 安装

**实例 (Instance)**：
一次「用哪个版本的 DSH、开哪个 DSH_HOME、用哪个 Profile」的绑定。实例是启动器
的调度单位；DSH 自己不知道「实例」这个词。
_Avoid_: 环境、配置

**绑定 (Binding)**：
实例的本质。一份版本、一个 DSH_HOME、一个 Profile，三者都由实例记录指向、都不由
实例拥有。删掉实例就是删掉这条绑定（ADR-0003）；版本与 DSH_HOME 原样留着。
_Avoid_: 实例数据、实例目录
_Avoid_: 环境、配置

**DSH_HOME**：
DSH 的数据目录，profile、会话历史与凭据都在它下面。可以多实例共用（如 `~/.dsh`），
也可以每个版本一份专属目录。
_Avoid_: 数据目录、home

**专属 DSH_HOME**：
位于 `<数据目录>/homes/<版本名>` 的 DSH_HOME，一个版本一个。它是启动器唯一能证明
属于自己创建的路径——别的 DSH_HOME 都可能装着用户自己的数据。
_Avoid_: 独立 home、私有目录

**Profile**：
DSH_HOME 下的一个插件组合，决定装载哪些插件、按什么顺序、各自怎么配。是 DSH 的
概念，不是启动器的。
_Avoid_: 配置、预设

**补丁文档 (Patch document)**：
Profile 里承载「用户自己怎么改这个组合」的那份 YAML，文件名 `cordis.patch.yml`。
DSH 原生写它（插件条目挂在里面）；启动器**不写**（ADR-0001），只读它来推导启动
这件事需要知道的一件事：端口钉在哪。
_Avoid_: patch 文件、配置覆盖

**Bundle（补丁层）**：
一份可以整体挂上或卸下的插件补丁层。挂在 `package.json` 的 `dsh.profile.bundles`
列表里。卸载它（原生 `removeBundle`）会取消选择它带来的整层配置，并且——如果这个包是
Profile 装的依赖——连那条依赖一起 `pnpm remove` 掉；installation 自带的层没有依赖可
移除，只取消选择。磁盘上 `link:` 指向的源码目录不归这一步管。
_Avoid_: 插件包、套件

**插件条目 (Plugin entry)**：
Profile 组合树里的一个可寻址节点，由 id 标识。一个 Bundle 通常插入若干条目。
条目的启停写在补丁文档里，跟它属于哪个 Bundle 无关。
启动器不给条目做控制面：一行补丁层只报「它带了多少条目」（ADR-0001）。
_Avoid_: 插件、模块

**依赖 (Dependency)**：
`package.json` 的 `dependencies` 里的一行，即「磁盘上装了哪个包」。装了不等于
挂载，摘了 Bundle 也不等于卸载。
_Avoid_: 已安装插件

**原生插件管理 (Native plugin management)**：
DSH 进程内提供的插件管理服务——列清单、启停、安装、卸载都在这里。它要求 DSH
正在运行。
_Avoid_: 网页端、GUI

**热重载 (HMR)**：
Profile 支持「改了立刻生效」而不是「下次启动才生效」的能力。官方 `web` 模板默认
装着（`packages/bundle/base` 的 `hmr` 行）。DSH 用它决定一次改动回 `applied` 还是
`restart-required`；后者发生时启动器页面上留一条「改动要重启才生效」。
_Avoid_: 热更新、live reload

**卸载 (Uninstall)**：
只用于插件——把一个补丁层连同它的依赖从这个 Profile 移除（原生 `removeBundle`，界面与
DSH 自带插件页同一个词、同一个方法）。启动器不再改叫「摘掉」：那会让人以为依赖留下。
_Avoid_: 删除（除非在说版本）

**删除版本 (Remove version)**：
把一个 DSH 版本从启动器里移除，连同它的实例。
_Avoid_: 卸载

**资源库 (Library)**：
顶级页面之一，放「实例由什么组成」的三样东西：DSH 版本、DSH_HOME、Profile。
它不是一个领域概念，是这三个概念在界面上共用的一个入口（ADR-0004）；三个词各自
的含义不变。
_Avoid_: 资料库、设置

**自检 (Check)**：
依赖树预检（`doctor`）。它是建议性的：发现的问题上报、记日志、不阻止启动。
_Avoid_: 健康检查、诊断
