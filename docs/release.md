# 发版（CI 自动化）

`.github/workflows/ci.yml` 负责构建与发布。日常发版只推一个 tag：

```bash
git tag v<version> && git push origin v<version>
```

- tag 必须等于 `package.json` 的 version，`ci/resolve-release.sh` 不匹配直接报错。
- 推 tag → 质量门禁 → 打包 aarch64 dmg → 发正式 release（Latest）；收尾的
  `bump-version` job 把 manifest 抬到下一个 patch 并推回 main。
- 推 main → 同样流程，但发预发布 `v<version>-dev.<工作流序号>`，不动 Latest。
- Release notes 手写（中英双语）。流水线只在正文还是占位符时填入「下载表 + 提交
  列表」，已写好的 notes 重跑不会被覆盖。改 `ci/release-notes-*.mjs` 后跑
  `pnpm test:release-notes`。
- `cargo fmt --check` 在 CI 是提示性的（`continue-on-error`），不阻塞发布。
- 发版 dmg 未签名未公证（只有本机自测副本走证书签名），用户侧的授权不跨版本保留；
  要改需要 Developer ID 证书（付费）并配到 CI。

抬版本用 `node ci/bump-version.mjs v<已发布的tag>`，改完跑 `node ci/check-versions.mjs`
校验三处一致（package.json / tauri.conf.json / Cargo.toml）。

CI 挂掉时手工补发：`pnpm tauri build --bundles dmg`，再
`gh release create v<版本> <dmg路径> --title v<版本> --notes-file <notes>`。

推送前本地复现质量门禁：

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace --locked
node ci/check-versions.mjs
```
