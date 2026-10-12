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
DSH 原生和启动器都写它，这是两者唯一的交集面。
_Avoid_: patch 文件、配置覆盖

**Bundle（补丁层）**：
一份可以整体挂上或摘下的插件补丁层。挂在 `package.json` 的 `dsh.profile.bundles`
列表里；摘掉它，它带来的整层配置一起消失，但依赖还留着。
_Avoid_: 插件包、套件

**插件条目 (Plugin entry)**：
Profile 组合树里的一个可寻址节点，由 id 标识。一个 Bundle 通常插入若干条目。
条目的启停写在补丁文档里，跟它属于哪个 Bundle 无关。
_Avoid_: 插件、模块

**依赖 (Dependency)**：
`package.json` 的 `dependencies` 里的一行，即「磁盘上装了哪个包」。装了不等于
挂载，摘了 Bundle 也不等于卸载。
_Avoid_: 已安装插件

**原生插件管理 (Native plugin management)**：
DSH 进程内提供的插件管理服务——列清单、启停、安装、卸载都在这里。它要求 DSH
正在运行。
_Avoid_: 网页端、GUI

**启动器插件管理 (Launcher plugin management)**：
启动器自己实现的插件管理。它试图在 DSH 没运行时也能改 Profile，是「原生插件
管理」之外的第二套实现。
_Avoid_: 启动器插件页

**热重载 (HMR)**：
Profile 支持「改了立刻生效」而不是「下次启动才生效」的能力。官方 `web` 模板默认
开着；关掉它的 Profile 只有重启才认配置改动。
_Avoid_: 热更新、live reload

**卸载 (Uninstall)**：
只用于插件——把插件及其依赖从某个 Profile 移除。
_Avoid_: 删除（除非在说版本）

**删除版本 (Remove version)**：
把一个 DSH 版本从启动器里移除，连同它的实例。
_Avoid_: 卸载
