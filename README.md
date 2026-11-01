# dsh-launcher

DeepSeek Harness（DSH）的桌面启动器。它把多份 DSH 版本、多个运行实例和各自的配置放进一个窗口里管理，省去手动切换目录、拼环境变量。

## 能做什么

- **主机**：管理本机，也可以用 ssh 管理远程机器；每台主机拥有自己的版本、DSH_HOME、Profile 和实例。
- **版本**：列出、安装和删除 DSH 版本，并指定哪个版本作为终端里的 `dsh` 命令。
- **DSH_HOME**：创建独立的 DSH_HOME 目录，让不同实例的数据互不干扰。
- **Profile**：新建、复制、改名和删除 Profile，给实例套用不同的设置。
- **实例**：新建、复制、启动、停止、重启和删除实例；查看运行状态与依赖自检结果，打开实例窗口和终端。
- **插件**：在实例详情页的「插件」标签里查看和调整该实例使用的插件。
- **设置**：调整启动器的全局选项。

## 界面

- **实例**：所有机器上的实例按状态排列；启动、打开、停止、重启和进入详情都在这一页完成。
- **安装**：按机器查看已安装与可安装的 DSH 版本，安装、删除版本，或把某个版本设为终端里的 `dsh` 命令。
- **数据**：管理每台机器的 DSH_HOME（数据目录）与其中的 Profile（配置）。
- **机器**：添加、编辑、测试和删除本机与远程主机，查看每台机器的运行环境。
- **设置**：主题、窗口与启动、日志、代理、更新和数据目录等启动器选项。

## 技术栈

- 前端：Vue 3、TypeScript、Vite、Pinia、Vue Router、Arco Design Vue
- 桌面端：Tauri 2（Rust）
- 包管理：pnpm

## 环境要求

- macOS 12.0 或更高版本
- Node.js 与 pnpm（用 `corepack enable` 启用 pnpm）
- Rust 工具链（通过 [rustup](https://rustup.rs) 安装，需要 rustfmt 和 clippy 组件）

## 开发

```bash
pnpm install       # 安装依赖
pnpm tauri-dev     # 启动开发模式，前端热更新并打开桌面窗口
pnpm dev           # 只启动前端页面（http://localhost:1420）
```

## 构建

```bash
pnpm build         # 类型检查并打包前端
pnpm tauri build   # 打出桌面安装包（dmg）
pnpm app:local     # 本机构建并用开发证书签名，安装到 /Applications 供自测
```

## 合并前检查

```bash
pnpm verify          # 全部检查：格式化、clippy、Rust 测试、版本同步、单元测试、prettier、前端构建
pnpm verify:quick    # 只跑 clippy 与 Rust 测试
pnpm format          # 用 prettier 格式化代码
```

## 目录结构

```
src/          Vue 前端（页面、组件、状态、接口）
src-tauri/    Tauri 桌面端（Rust 后端与打包配置）
scripts/      构建、校验等辅助脚本
```