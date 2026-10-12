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
  npm 的版本会走源码克隆构建）。同一版本可建多个实例，各自拥有独立的名称、Profile
  与环境变量覆盖；`DSH_HOME` 可共用（如 `~/.dsh`，启动器会自动收录）或使用实例专属目录。
- **环境检测**：自动识别本机已有的 Node / pnpm（覆盖 nvm、fnm、Homebrew、官方安装
  脚本的目录布局）并绑定绝对路径；缺环境时按本机已有的版本管理器给出一条可复制的
  安装命令，装完切回窗口即自动重新检测。
- **一键启动**：选实例 + Profile 启动，DSH 的 Web GUI **在系统浏览器中打开**；也可
  直接为实例打开外部终端（Terminal.app 或 Ghostty），带上同一套环境变量。
- **实例健康检查**：按实例实际使用的版本与 Profile 做依赖自检，把依赖树异常以告警 /
  错误提示出来（不阻塞启动），并可分级着色查看实例日志。
- **插件管理**：在 Profile 页的某一行点「管理插件」进入，列出该 Profile 的补丁层
  （Bundle，带版本号）与插件条目，每一条插件条目后面也带它所属补丁层的 `@版本`；
  支持挂载 / 摘掉 / 卸载与启用 / 禁用。开关走 DSH
  自己的插件管理接口，改动是否生效以实例返回的结果为准；实例未运行时只读展示。
- **安装进度就地可见**：点一个版本即开始安装，进度与「取消安装」直接显示在那一行上。
  安装成功后，它专属的 DSH_HOME 与对应的 1:1 实例一并建好；没有单独的任务页要来回切。
- **系统托盘**：左键打开主窗口，右键打开菜单（运行状态、实例直达与停止、一键启停、
  目录 / 日志 / 更新 / 设置）；退出时自动清理全部实例进程。
- **快捷键**：`⌘1`–`⌘4` 在首页 / HOME / Profile / 版本间跳转，`⌘,` 设置，
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
- `versions/<版本>/`：各版本隔离安装目录
- `homes/<名称>/`：实例专属 DSH_HOME
- `logs/`：启动器与实例运行日志
- `scripts/`：为「打开终端」生成的临时包装脚本
- `.pnpm-store/`：启动器自有的 pnpm 内容 store

卸载应用不会删除这个目录；需要彻底清理时手动删除即可。

## 架构

- **前端** `src/`：Vue 3（页面、store、composables、vue-i18n）。`src/api/` 是唯一的
  invoke seam——`index.ts` 定义公共 interface，`tauri.ts`（桌面）与 `mock.ts`（浏览器
  预览，同时充当后端契约的参考实现）是两个 adapter，`ci/api-contract.test.mjs` 防止
  两边漂移。路由用 hash history；`src/shortcuts.ts` 是快捷键与后端菜单事件的唯一契约。
- **后端** `src-tauri/src/`：Rust。
  - `config` 配置持久化与迁移 · `commands` Tauri 命令面
  - `process` 实例生命周期（running 表）· `launch` DSH 调用规格（argv / env / PATH / 版本布局）
  - `profile` profile 目录与 `cordis.patch.yml` · `plugins` 插件安装与启停
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
