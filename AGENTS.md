# AGENTS.md — DSH Launcher

Tauri 2 + Vue 3 的 macOS Apple Silicon 桌面启动器：管理多版本 DSH 与多实例，
每个实例有独立的 `DSH_HOME`、Profile 和环境变量。运行数据在
`~/Library/Application Support/in.dsh-plug.dsh-launcher`（布局见 README「运行数据」）。

**打开实例 = 用系统浏览器打开实例 URL（契约，勿回退）**：经 `open that`，
与 `open_external` 同策略（先 trim 再只放行 http 与 https）。
`open_instance_window` 保留命令名和只传 id 的签名，后端从 running 表查 URL；
不存在任何实例 webview（回退史见 ADR-0002）。

## 日常开发

- 验证走 `pnpm tauri dev`（真桌面 + Rust 后端）；`pnpm dev` 只有浏览器
  localStorage mock。
- Rust 手工排版：全量 `cargo fmt` 会与并行 worktree 冲突，CI 的 `fmt --check` 不阻塞。

## 本机自测 App

自测副本固定在 `/Applications/dsh-launcher.app`。`pnpm tauri build --bundles app`
只产出 target 里的 `.app`；`pnpm app:local` 一步构建 + 签名 + 替换 + 校验
（`--no-build` 只补签 + 替换；会先退出运行中的实例）。

**签名必须用证书，ad-hoc 不行**：ad-hoc 的 designated requirement（DR）就是
`cdhash`，App 一重建，macOS 存的 TCC 授权（如「完全磁盘访问」）就失配——症状
隐蔽，系统设置里开关还开着、实际没权限。证书 DR 只钉 bundle id 和证书，重建后
不变；完整说明见 `ci/build-local-app.sh` 头注释，脚本发现 DR 含 cdhash 会直接报错退出。

- bundle id（`in.dsh-plug.dsh-launcher`）和安装路径别改，改了会重置授权。
- 证书约一年到期，DR 钉的是证书 CN（不是 Team ID），换发新证书要重新授权一次。

## 发版

推 tag、抬版本、release notes、本地复现门禁、CI 挂掉补发 → `docs/release.md`。

## Agent skills

- **Issue tracker**：issues 与 specs 都是 GitHub issue（`tttnny/dsh-launcher`），
  用 `gh` CLI 操作 → `docs/agents/issue-tracker.md`
- **Triage labels**：五个 triage 角色的标签串与角色名完全一致 →
  `docs/agents/triage-labels.md`
- **Domain docs**：单 context——根 `CONTEXT.md` 词汇表 + `docs/adr/` 系统级 ADR →
  `docs/agents/domain.md`
