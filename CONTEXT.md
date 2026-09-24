# DSH Launcher

macOS 桌面启动器：管理多版本 DSH 与多实例，每个实例有独立的 DSH_HOME、Profile
与环境变量。唯一把 DSH 进程、插件市场、版本安装和托盘编排到一起的地方。

## Language

### 版本与安装

**DSH**：
DeepSeek Harness CLI，从 npm（或 GitHub 源码 tag）安装的受管运行时。
_Avoid_: 内核、core

**版本（Version）**：
一个隔离安装到 `versions/<版本>` 的 DSH 发行版。npm 树或源码 checkout 两种布局。
_Avoid_: 发行版、release

**装版本 ≡ 建实例**：
安装一个版本必然同时创建它的 1:1 实例与专属 HOME——见 ADR-0001。

### HOME 与实例

**HOME（DSH_HOME）**：
一个 DSH 数据目录：profiles、会话与插件都住在里面。可共用（如 `~/.dsh`）或专属。
_Avoid_: 家目录、home 目录（指 HOME 时统一大写）

**实例（Instance）**：
名称 + 版本 + HOME + 环境变量覆盖 + 可选固定端口的组合。启动的最小单位。
_Avoid_: 应用、容器

**专属 HOME（dedicated HOME）**：
随实例创建的 `<数据目录>/homes/<名>` HOME；下载任务先占位路径，成功才落记录。

### Profile 与插件

**Profile**：
HOME 下 `profiles/<名>` 的依赖集合。三种行归属见 **Profile Patch 文档**。
_Avoid_: 配置文件（指 profile 时）

**Profile Patch 文档**：
`cordis.patch.yml`：CLI 写 insert 行挂载插件，启动器写 disabled 行开关插件，
remote-web-ui 插件管理带标记的 lan-bind 块（端口钉扎，级别高于 CLI `--port`）。
_Avoid_: 补丁文件

**插件渠道**：
稳定版 / 测试版 / 最新提交三种安装渠道，装到实例 Profile。

**`__temp__` 模板**：
生成新 profile 的引导模板；其 patch 文档中 webserver 端口钉扎被清零。

### 运行时

**生命周期（Instance Lifecycle）**：
实例的 start / stop / restart / 收养 / 退出状态机；running 表唯一 owner 的
那套迁移（starting → running → stopped/exited）。
_Avoid_: 进程管理（泛指时用生命周期）

**收养（Adoption）**：
DSH 自重启后占住固定端口的外部进程被登记为运行中实例，靠 pid 存活轮询看护。

**后台任务（Task）**：
版本安装 / Node 安装一类长时操作；有 Running / Done / Error / Cancelled 状态。

**工具链（Toolchain）**：
"哪个 pnpm（主版本钉 11）、哪个 store、哪个 registry、哪些网络 flags" 的唯一裁决。

**启动规格（Launch Spec）**：
"这个用途下如何调用 DSH"——argv（NODE_RUNTIME_FLAGS 顺序、`--host` 去留、
`--no-open` 探测）、env 策略、终端 PATH。四种调用形态：实例运行 / 模板引导 /
插件管理 / 终端。
