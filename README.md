# dsh-launcher

DeepSeek Harness（DSH）的桌面启动器。它把多份 DSH 版本、多个运行实例和各自的配置放进一个窗口里管理，省去手动切换目录、拼环境变量。

## 能做什么

- **主机**：管理本机，也可以用 ssh 管理远程机器；每台主机拥有自己的版本、DSH_HOME、配置档和实例。
- **版本**：列出、安装和删除 DSH 版本，并指定某个版本实际指向的文件。
- **DSH_HOME**：创建独立的 DSH_HOME 目录，让不同实例的数据互不干扰。
- **配置档**：新建、复制、改名和删除配置档，给实例套用不同的设置。
- **实例**：新建、复制、启动、停止、重启和删除实例；查看运行状态与健康日志，打开实例窗口和终端。
- **插件**：在实例的展开卡片里打开插件管理，查看和调整该实例使用的插件。
- **设置**：调整启动器的全局选项。

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
