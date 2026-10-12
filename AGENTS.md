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

推 tag 即发版（`git tag v<version> && git push origin v<version>`，tag 必须等于
`package.json` 的 version）。推 main 发预发布 `v<版本>-dev.<工作流序号>`，不动
Latest；收尾的 `bump-version` job 把 manifest 抬到下一个 patch 并推回 main。

- Release notes 手写（中英双语）；流水线只在正文还是占位符时填「下载表 + 提交
  列表」，改 `ci/release-notes-*.mjs` 后跑 `pnpm test:release-notes`。
- 发版 dmg 未签名未公证（只有本机自测副本走证书签名）；要改需 Developer ID
  证书（付费）并配到 CI。
- CI 挂掉时手工补发：`pnpm tauri build --bundles dmg`，再
  `gh release create v<版本> <dmg路径> --title v<版本> --notes-file <notes>`。

推送前本地复现质量门禁：

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace --locked
node ci/check-versions.mjs
```

## Agent skills

### Issue tracker

issue 与 spec 都是 GitHub issue（`tttnny/dsh-launcher`），用 `gh` CLI 操作 →
`docs/agents/issue-tracker.md`

### Triage labels

triage 的五个角色用中文标签串（`需要分诊`、`需要补充信息`、`待Agent处理`、
`待人工处理`、`不处理`）→ `docs/agents/triage-labels.md`

### Domain docs

单 context：根 `GLOSSARY.md` 词汇表 + `docs/adr/` 系统级 ADR →
`docs/agents/domain.md`
