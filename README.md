<div align="center">

# DSH Launcher

**多版本、多实例的 [DeepSeek Harness (DSH)](https://github.com/deepseek-ai/deepseek-harness) macOS 桌面启动器**

Tauri 2 + Vue 3 + TypeScript，Apple Silicon。

</div>

## 这是什么

DSH 本身是命令行工具，跑起来还要挑版本、配 `DSH_HOME`、选 Profile、管插件。同时用
几个版本、几套数据目录时，手工维护很快就乱了。

DSH Launcher 把这些收进一个桌面应用：从 npm 安装多个 DSH 版本到互相隔离的目录，
每个版本建独立实例与数据目录，启动器负责挑 Profile、注入环境变量、拉起进程、把
Web GUI 交给你，并在退出时收拾干净。

## 功能

- **多版本 / 多实例**：从 npm 安装多个 DSH 版本到隔离目录（仅 GitHub 发布、未上
  npm 的版本会走源码克隆构建）。安装会为版本建第一个实例；在实例页可以再新建任意
  多个实例，也可以在实例行的展开卡片里用「复制实例」起一个副本。各自有独立名称、
  Profile 与环境变量覆盖，也可以绑到不同的 `DSH_HOME`。
  实例是一条**绑定记录**：删掉实例不会动版本和 DSH_HOME（见 ADR-0003）。
- **环境检测**：自动识别本机已有的 Node / pnpm（覆盖 nvm、fnm、Homebrew、官方安装
  脚本的目录布局）并绑定绝对路径；缺环境时按本机已有的版本管理器给出一条可复制的
  安装命令，装完切回窗口即自动重新检测。
- **一键启动**：在实例行上选 Profile 直接启动，DSH 的 Web GUI **在系统浏览器中打开**；
  运行中的实例行直接显示 URL 与「打开」按钮。点实例名展开卡片，里面是这台实例的全部
  事实与动作：版本 / DSH_HOME / 默认 Profile / 端口 / 环境变量覆写 / 自检结果，以及
  打开终端（Terminal.app 或 Ghostty，带同一套环境变量）/ 目录 / 日志 / 复制实例 /
  重新读取 Profile 列表 / 实例设置。
- **实例自检**：按实例实际使用的版本与 Profile 做依赖树预检，把异常以告警 / 错误
  提示出来（不阻塞启动），侧栏的自检入口可分级查看、复制与重新检测。
- **插件管理**：插件归**运行中的实例**管，入口在实例页（详情页头部按钮，或实例行
  展开卡片里的按钮），带实例 + DSH_HOME + Profile 三级作用域；「返回实例」回到你离开
  的地方——从实例行走就回到那一行（卡片自动展开、滚到位），从详情页走就回详情页。
  页面管的是**补丁层**（Bundle，带版本号）：挂载 / 停用、以及把自己加的层**卸载**；
  层里带了多少插件条目只报个数，条目不在这里一条条开关（那属于 DSH 自己的界面）。
  列表按归属分两段——「你添加的」常开，「随 DSH 发布」折叠起来并标着数量；挂没挂载看
  开关与状态点，不另设分类（刚切掉的那一行不会从你手下跑走）。开关走
  DSH 自己的插件管理接口，改动是否生效以实例返回的结果为准；实例未运行或中途停掉时，
  页面只留一句原因与「启动 / 重启实例」的按钮，不做只读的假列表。
- **安装进度就地可见**：点一个版本即开始安装，进度与「取消安装」直接显示在那一行上。
  安装成功后，它专属的 DSH_HOME 与对应的 1:1 实例一并建好；没有单独的任务页要来回切。
- **系统托盘**：左键打开主窗口，右键打开菜单（运行状态、实例直达与停止、一键启停、
  目录 / 日志 / 更新 / 设置）；退出时自动清理全部实例进程。
- **快捷键**：`⌘1` 实例、`⌘2` DSH 版本、`⌘3` DSH_HOME、`⌘4` Profile，`⌘,` 设置，
  `⌘0` 显示主窗口，`⌘R` 刷新，`Esc` 返回，另有 `⌘Q/H/M/W` 等原生菜单操作。
- **其他**：关闭主窗口最小化到托盘、开机自启、中英双语界面、浅色 / 深色 / 跟随系统
  主题、日志级别可调、代理设置、`dsh-launcher://launch` 深链启动、应用内自更新检查。

## 环境要求

运行 DSH 实例需要 **Node.js ≥ 20**（DSH 未声明 `engines`，20 是建议下限，更低只告警不
阻止）。安装版本与管理插件还需要 **pnpm ≥ 11**。

工具链由你自己提供——nvm / fnm / Homebrew / 官方安装脚本都行。启动器**不会**往你的磁盘
上再装一份 Node 或 pnpm，只做探测与绑定；绑定结果可在设置页随时查看。

应用本身要求 **macOS 12+**（Apple Silicon）。

## 安装

从 [Releases](https://github.com/tttnny/dsh-launcher/releases) 下载最新的
`dsh-launcher_<版本>_aarch64.dmg`，打开后把应用拖进「应用程序」。

> 发布用的 dmg 未签名、未公证，首次打开需在「系统设置 → 隐私与安全性」中放行，
> 或右键应用选择「打开」。

## 开发

前置：Node ≥ 20、pnpm ≥ 11、Rust stable（Xcode Command Line Tools）、macOS 12+。

```bash
pnpm install        # 安装依赖
pnpm tauri dev      # 开发模式：Vite 前端 + 真实 Rust 后端（真桌面）
pnpm tauri build    # 打包：Apple Silicon .dmg
pnpm dev            # 仅浏览器预览：localStorage mock，无 Rust 后端
```

验证功能请走 `pnpm tauri dev`——`pnpm dev` 只有浏览器里的 mock 实现，测不到真实行为。

### 本机自测 App

`pnpm app:local` 一步完成构建 + 签名 + 替换 + 校验，把自测副本装到
`/Applications/dsh-launcher.app`（`--no-build` 只补签与替换，会先退出运行中的实例）。

签名必须用证书，ad-hoc 不行：ad-hoc 的 designated requirement 就是 `cdhash`，应用一
重建，macOS 存的 TCC 授权（如「完全磁盘访问」）就失配——症状隐蔽，系统设置里开关还
开着、实际却没权限。证书签名的 DR 只钉 bundle id 与证书，重建后不变。详见
`ci/build-local-app.sh` 头注释。

- bundle id（`in.dsh-plug.dsh-launcher`）与安装路径不要改，改了会重置授权。
- 证书约一年到期，换发新证书后需重新授权一次。

## 运行数据

数据目录：`~/Library/Application Support/in.dsh-plug.dsh-launcher/`

- `config.json`：DSH_HOME / 版本 / 实例 / 设置
- `running.json`：实例就绪时打印的 URL（含启动 token），用于启动器重启后接管仍在运行的实例
- `versions/<版本>/`：各版本隔离安装目录
- `homes/<版本名>/`：版本专属 DSH_HOME（安装版本时创建；删除版本时按路径删除，见 ADR-0002）
- `<DSH_HOME>/icons/<实例 id>.png`：实例自定义图标（随实例删除）
- `logs/`：启动器与实例运行日志
- `scripts/`：为「打开终端」生成的临时包装脚本
- `.pnpm-store/`：启动器自有的 pnpm 内容 store

卸载应用不会删除这个目录；需要彻底清理时手动删除即可。

## 界面

顶级导航按「你要做什么」分三个目的地（ADR-0004）：

- **实例**：实例的看板。运行中的实例带状态轨与实时 URL，行内选 Profile 后启动。
  点这一行（或实例名）展开卡片：上半是事实（版本 / DSH_HOME / 默认 Profile / 端口 /
  环境变量覆写 / 自检），下半是动作，按作用对象分三段——进程态（重启 / 停止 / 管理
  插件，只有运行中的实例有）、机器（终端 / 目录 / 日志）、绑定（复制实例 / 重新读取
  Profile 列表），末尾是实例设置。行上既不藏溢出菜单，也不堆图标按钮。这里是新建与
  删除实例的地方。
- **资源库**：实例由什么组成——DSH 版本、DSH_HOME、Profile 三个标签页。Profile 行
  只陈述事实（哪些实例默认用它），改名 / 复制 / 删除在这里，插件管理不在这里。
- **设置**：启动器自身——外观、语言、托盘、代理、更新、数据目录、快捷键与环境。

## 架构

- **前端** `src/`：Vue 3（页面、store、composables、vue-i18n）。`src/api/` 是唯一的
  invoke seam——`index.ts` 定义公共 interface，`tauri.ts`（桌面）与 `mock.ts`（浏览器
  预览，同时充当后端契约的参考实现）是两个 adapter，`ci/api-contract.test.mjs` 防止
  两边漂移。路由用 hash history；`src/shortcuts.ts` 是快捷键与后端菜单事件的唯一契约。
- **后端** `src-tauri/src/`：Rust。
  - `config` 配置持久化与迁移 · `commands` Tauri 命令面
  - `process` 实例生命周期（running 表）· `launch` DSH 调用规格（argv / env / PATH / 版本布局）
  - `profile` profile 目录与 `cordis.patch.yml` · `plugins` 补丁层的列表 / 启停 /
    卸载（只走运行中实例的 `pluginManager` HTTP，启动器不写 profile 文件）
  - `toolchain` pnpm / registry / 网络参数 · `runtime` Node / pnpm 探测与工具链绑定
  - `tasks` 后台任务 runner（目前只有安装版本一种）· `doctor` 实例健康检查 · `proxy` 代理注入
  - `tray` 系统托盘 · `app_menu` 原生菜单栏 · `windows` 窗口管理 · `update` 自更新检查

`AGENTS.md` 记录了开发约定与几处不可回退的契约（例如「打开实例 = 用系统浏览器打开
实例 URL」）。

## 发版

推 tag 即触发 CI 构建与发布：

```bash
git tag v<version> && git push origin v<version>
```

tag 必须与 `package.json` 的 version 一致（`ci/check-versions.mjs` 校验三处 manifest），
推 tag 发正式版，推 `main` 发预发布 `v<版本>-dev.<工作流序号>`。抬高版本用
`node ci/bump-version.mjs v<已发布的 tag>`。

## License

MIT
