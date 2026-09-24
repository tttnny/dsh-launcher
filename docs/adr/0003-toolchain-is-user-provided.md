# 工具链由用户提供，启动器只探测与绑定

## 背景

启动器需要 `node`（运行 DSH 进程）与 `pnpm`（初始化 profile、装版本、管插件）。
历史上它的做法是两件事叠加：

1. **自己装 Node**：`start_install_node_task` 下载官方 LTS tar.gz 解压到
   `<数据目录>/tools/node`，再引导安装 pnpm 到 `<数据目录>/tools`；
2. **钉死 pnpm 主版本 11**：`REQUIRED_PNPM_MAJOR = 11`，认为「DSH profile 由
   pnpm 11 初始化，其他主版本会产生 CLI 不期望的依赖树」。

这两条在新机器上都失效了：

- 引导安装 pnpm 后的存在性检查找的是 `<tools>/pnpm`，而 macOS 上 npm 的全局
  bin 目录是 `<prefix>/bin`（Windows 才是 prefix 根），所以「一键装 Node」在
  没有系统 pnpm 的机器上必定以「pnpm 安装完成但未找到可执行文件」失败；
- 各条易得渠道给出的 pnpm 都已是 12（`brew install pnpm` 12.5.1、
  `npm i -g pnpm` 12.6.0、`curl get.pnpm.io` 12.x），精确钉 11 等于拒绝用户
  已经装好的工具链；
- 而探测层也有对应缺陷：官方 pnpm 安装脚本把可执行文件放在
  `~/Library/pnpm/bin/pnpm`，启动器只探 `~/Library/pnpm/pnpm`；nvm 的
  `~/.nvm/versions/node/*/bin` 从未被扫描。

关于钉 11 的前提，实测不成立：DSH 的 profile 初始化只写 `package.json`、
`cordis.patch.yml`、`pnpm-workspace.yaml`（`nodeLinker: hoisted`），既不设
`packageManager` 也不做版本门禁；DSH 仓库里的 `pnpm@11.7.0` 是构建 DSH 自己
monorepo 的开发依赖。pnpm 11 与 12 生成的 lockfile 字节相同，两代都能
`--frozen-lockfile` 交叉安装，`allowBuilds` / `ERR_PNPM_IGNORED_BUILDS` 流程
在两代上逐字一致。

## 决策

**1. 工具链由用户提供，启动器不再安装，只探测并绑定。**

「装 Node」「装 pnpm」占用用户磁盘、与 nvm/fnm/brew 抢位置，且每一步都要跟着
上游版本漂移维护（本项目自己就坏过一次）。改为：启动器探测用户已有的工具链，
在引导页给出**贴合本机**的可复制命令（检测到 nvm 就给 `nvm install --lts`，
检测到 fnm/brew 同理），用户自己在终端装，切回窗口即自动重新检测。

**2. pnpm 接受范围改为 `>= 11`，不再是精确的 11。**

下限保留 11（启动器自己的 flag 集与 store 布局验证过的下限），上限放开。任何
≥ 11 的 pnpm 都直接采用，因此 brew/npm 默认装到的 12 不再被拒绝。

**3. 与放宽同时，pnpm 参数改用 `--config.` 拼法。**

pnpm 12 的 CLI 重写（Rust/clap）删掉了旧的裸拼法，迁移是放宽的前提而非附赠：
`--fetch-retries` 报 "unexpected argument"，`--loglevel=http` 报
"invalid value 'http'"（后者出现在版本安装主路径上，会让装版本直接失败）。
`--config.<name>=<value>` 两代都接受。`--network-concurrency` 没有两全拼法
（12 要 `--config.`，而 11 用 `--config.network-concurrency=4` 会崩），故直接
去掉，用 pnpm 的默认并发。

**4. 解析结果持久化到 settings，作为绝对路径传进启动契约。**

`settings.toolchain` 记录 node/pnpm 的绝对路径与版本。历史问题是一切 spawn 都
按名字（`Command::new("node")`）解析，依赖启动那一刻的 PATH，于是「在终端装完
Node → 不重启 app 也用不上」。现在启动时探测一次并写入，`launch.rs` 的
`dsh_base` 取显式传入的路径。绑定是**探测缓存而非用户偏好**：路径失效（nvm 换
版本、卸载 brew 包）就重新探测并更新，只在完全找不到时报 actionable 错误。

**5. 探测统一到一个候选列表，并固定中性 cwd。**

`runtime::candidate_paths` 是唯一来源，UI 展示与真正 spawn 用它取得一致。探测
`--version` 时 cwd 固定为 `/`：pnpm 会读取 cwd 下 `package.json` 的
`packageManager` 并转派（corepack shim 甚至会触发下载），同一个 pnpm 12 二进制
在不同 cwd 会分别报 `12.6.0` 与 `9.0.0`，从 app 自身 cwd 探测会得到不真实的
版本号。

## 后果

- 新机器上「装 Node」不再是一键：用户需在终端跑一条命令。换来的是不必在数据
  目录里多存一份 Node，也不覆盖用户用 nvm/brew 管理的版本。
- `node` 缺失是硬失败（DSH 无法启动）；`pnpm` 缺失只在装版本与插件管理时报错，
  错误文案直接给出可复制命令。
- 「环境已就绪」以可用性为准：node 存在即可启动，pnpm 只在这一页提示。Node 低
  于 20 仅黄标警告（DSH 的 `engines` 为空，README 的 ≥20 是开发前置）。
- 引导页（`/setup`）从「仅在缺 Node 时自动跳入」变为可从设置页随时进入的状态
  页，显示绑定到的绝对路径。
- HOME 路径输入统一走 `config::normalize_dir_path`：展开 `~`、拒绝相对路径、
  去尾斜杠。此前填 `~/.dsh` 会在 app 当前工作目录下建一个字面名为 `~` 的目录。

## 备选方案

- **保留一键安装 Node 作为兜底**：被否。它刚被证实是坏的，且与「不覆盖用户
  工具链」的目标冲突。
- **保留精确钉 11**：被否。前提不成立（DSH 不钉版本），而代价是所有走默认渠道
  装 pnpm 的用户都被拒绝。
- **探测不落盘、每次现场解析**：被否。每个实例启动都多一次文件系统扫描，且
  「终端里装完立刻可用」这一诉求需要一份可刷新的绑定。
- **用 `runtime::candidate_paths` 之外再维护一份 spawn 用列表**：被否。两份列表
  漂移后会出现「界面说已装、启动却找不到」。
