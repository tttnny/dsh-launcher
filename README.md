# DSH Launcher

多版本、多实例的 [DeepSeek Harness (DSH)](https://github.com/deepseek-ai/deepseek-harness) macOS 桌面启动器。Tauri 2 + Vue 3 + TypeScript，面向 Apple Silicon。

## 这是什么

DSH 是命令行工具。同时使用几个版本、几套数据目录时，手工挑选版本、配置 `DSH_HOME`、选择 Profile、管理插件很容易乱。

DSH Launcher 把这些收进一个桌面应用：从 npm 安装多个互相隔离的 DSH 版本，为每个版本建立实例与数据目录，负责选择 Profile、注入环境变量、拉起进程，并把 DSH 的 Web GUI 交给系统浏览器。退出时自动清理全部实例进程。

## 主要功能

- **多版本、多实例**：版本安装在各自独立的目录里，可并存、各自升级。每个实例是一条绑定记录，指向一份版本、一个 `DSH_HOME`、一个 Profile，三者都是同一台主机上的东西。
- **登记到 PATH**：把一个版本登记为所在机器上的 `dsh` 命令，供只按命令名查找工具的程序使用。
- **环境检测**：以系统环境变量为基准查找 Node / pnpm，说明来源；也可以用指定的可执行文件覆盖探测结果。
- **一键启动**：在实例行选择 Profile 启动，DSH 的 Web GUI 在系统浏览器中打开，运行中的实例行显示地址与「打开」按钮。
- **插件管理**：管理运行中实例的补丁层（挂载、停用、卸载），实例未运行时展示该 Profile 装了什么。
- **系统托盘、快捷键、中英双语、浅色 / 深色主题**。

## 环境要求

- 运行 DSH 实例需要 **Node.js ≥ 20**，安装版本与管理插件需要 **pnpm ≥ 11**。工具链由你自己提供，启动器只做探测与绑定，不会另装一份。
- 应用本身要求 **macOS 12+**（Apple Silicon）。

## 快速开始

前置：Node ≥ 20、pnpm ≥ 11、Rust stable（Xcode Command Line Tools）、macOS 12+。

```bash
pnpm install      # 安装依赖
pnpm tauri dev    # 开发模式：Vite 前端 + 真实 Rust 后端
pnpm tauri build  # 打包 Apple Silicon .dmg
pnpm dev          # 仅浏览器预览：localStorage mock，无 Rust 后端
```

验证功能请用 `pnpm tauri dev`：`pnpm dev` 只有浏览器里的 mock，测不到真实行为。

打出来的 dmg 未签名、未公证，首次打开需在「系统设置 → 隐私与安全性」中放行。要在本机装一份证书签名的自测副本，用 `pnpm app:local`。

## 运行数据

数据目录：`~/Library/Application Support/in.dsh-plug.dsh-launcher/`

- `config.json`：主机、DSH_HOME、版本、实例与设置
- `running.json`：运行中实例的地址与启动 token，用于启动器重启后接管
- `versions/`、`homes/`：各版本安装目录与版本专属 DSH_HOME
- `logs/`：启动器与实例日志

卸载应用不会删除这个目录，需要彻底清理时手动删除即可。

## 开发文档

- `AGENTS.md`：开发命令速查与规则文档索引
- `GLOSSARY.md`：术语表
- `docs/agents/rules/`：按主题拆分的规则
- `docs/adr/`：每条设计约定的完整推理

## License

MIT
