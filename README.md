<div align="center">

<img src="docs/banner.png" width="1024" height="512" alt="DSH Launcher">

# DSH Launcher

**多版本、多实例的 [DeepSeek Harness (DSH)](https://github.com/deepseek-ai/deepseek-harness) macOS 桌面启动器**（Tauri 2 + Vue 3，Apple Silicon）。

</div>

## 功能

- **多版本 / 多实例**：从 npm 安装多个 DSH 版本到隔离目录；同一版本可建多个实例，各自拥有独立名称、Profile 与环境变量（`DSH_HOME` 支持共用、自动收录 `~/.dsh` 或专属目录）。
- **环境检测**：自动识别本机已有的 Node / pnpm（覆盖 nvm、fnm、Homebrew、官方安装脚本的目录布局）并绑定绝对路径，缺环境时按本机情况给出可复制的安装命令；设置页可随时查看绑定结果。
- **一键启动**：选择实例 + Profile 启动，自动在独立窗口中打开 DSH Web GUI。
- **插件市场**：浏览 [dsh-plug.in](https://dsh-plug.in) 插件，按稳定版 / 测试版 / 最新提交三渠道安装到任意实例的 Profile，支持按 Profile 启用 / 禁用。
- **系统托盘**：左键打开主窗口，右键打开菜单（运行状态、实例直达与停止、一键启停、目录/日志/更新/设置）；双击打开最近使用的实例；退出时自动清理全部实例进程。
- **快捷键（macOS）**：`Cmd+1/2/3/4` 依次到首页/实例/任务/下载，`Cmd+,` 设置，`Cmd+R` 刷新，`Esc` 返回；原生菜单支持 `Cmd+Q/H/M/W` 等标准操作。
- **其他**：关闭窗口最小化到托盘、开机自启、中英双语界面。

## 环境要求

DSH 实例需要 **Node.js ≥ 20**（DSH 的 `engines` 未声明要求，20 是建议下限，
更低只警告不阻止）。安装 DSH 版本与管理插件还需要 **pnpm ≥ 11**。

工具链由你自己提供——用 nvm / fnm / brew / 官方安装脚本都行，启动器**不会**往
你的磁盘上再装一份，只做探测与绑定。首次启动时若环境不全，会进入「环境检测」
页（也可从设置页随时进入）：页面显示绑定到的 node / pnpm 绝对路径，并按本机已
有的版本管理器给出一条可复制的安装命令。装完切回窗口即自动重新检测。

## 开发

前置：Node ≥ 20、pnpm ≥ 11、Rust stable（Xcode Command Line Tools）、macOS 12+。

```bash
pnpm install        # 安装依赖
pnpm tauri dev      # 开发模式（前端 Vite + 后端 debug）
pnpm tauri build    # 打包（Apple Silicon .dmg）
pnpm dev            # 浏览器预览（localStorage mock）
```

## 运行数据

数据目录：`~/Library/Application Support/in.dsh-plug.dsh-launcher/`

- `config.json`：DSH_HOME / 版本 / 实例 / 设置
- `versions/<版本>/`：各版本隔离安装目录
- `homes/<实例名>/`：实例专属 DSH_HOME（如选择）
- `logs/`：启动器与实例运行日志

## 架构

- `src/`：Vue 3 前端（页面、store、composables、i18n）。`src/api/` 是唯一 invoke
  seam：`index.ts` 定义公共 interface，`tauri.ts`（桌面）与 `mock.ts`（浏览器预览，
  后端契约参考实现）是两个 adapter，`ci/api-contract.test.mjs` 防漂移。
- `src-tauri/src/`：Rust 后端——`config`（配置持久化）、`commands`（Tauri 命令）、
  `process`（实例生命周期：running 表、迁移与投影）、`launch`（DSH 调用规格：
  argv/env/PATH/版本布局）、`profile`（profile 目录与 cordis.patch.yml 文档）、
  `toolchain`（pnpm/registry/网络参数）、`tasks`（后台任务 runner）、`plugins`
  （插件市场与安装）、`runtime`（Node/pnpm 探测与工具链绑定）、`tray`（托盘）、`windows`
  （窗口管理）。领域词汇见 `CONTEXT.md`，架构决策见 `docs/adr/`。

## License

MIT
