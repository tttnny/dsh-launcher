# 开发、验证与打包

管每天要跑的那几件事：门禁、测试怎么写、排版、真机验证、自测 App 与打包。真机验证的
完整说明在 `docs/testing-on-a-real-host.md`（那台机器是什么、`#[ignore]`d 的测试怎么跑、
无窗口 harness 长什么样、在别人机器上不能留下什么），这一份只讲与它接壤的那几句。

## 门禁

- 提交前跑 `pnpm verify`：它聚合 `cargo fmt --check`、clippy、Rust 测试、版本同步、全部
  `ci/*.test.mjs`（通配符发现）、前端排版（`prettier --check`）与前端构建，与 CI 同一份
  名单——名单只有这一处，`ci/gate-list.test.mjs` 会盯住它没被抄成第二份。合并之间想快过
  一次用 `pnpm verify:quick`（只跑 clippy 与 Rust 测试）；想单独跑一个门禁，用对应的
  `pnpm test:*` 脚本。
- 手工点击验证走 `pnpm tauri dev`（真桌面 + Rust 后端）；`pnpm dev` 只有浏览器
  localStorage mock，测不到真实行为。

## 新增一条 Tauri 命令

要同时改三处，否则 `pnpm test:api-contract` 会失败：`src/api/index.ts`（interface）、
`src/api/mock.ts`（switch case）、`src-tauri/src/lib.rs`（`generate_handler!`）。**参数
拼写也是契约**：`#[tauri::command(rename_all = "snake_case")]` 的命令后端按 snake_case
读，前端发的名字对不上时 Tauri 不会报错——`Option<T>` 直接变成 `None` 走了默认值（真发生
过：远程安装在界面说「正在服务器上装」的同时装到了本机）。`ci/api-contract.test.mjs` 会
逐参数比对两边的拼写，并盯住 mock 每个分支读的键——浏览器 mock 与后端读的是同一把键，所以
拼错在预览里就是可见的错值（前端发的键与后端读的键，两处对不上的当场报出来）。

## 测试

测试断言**可观察的结果**——公开接口的行为、界面读到的内容；私有函数用真实输入测它的
产出，不测「内部调用了谁」（`assert_eq!(fake.calls(), vec![…])` 这种）。

前端没有测试框架（无 vitest/jsdom），能测的那几件事写成 `ci/*.test.mjs` 的源码级检查。
`ci/setup-guidance.test.mjs` 会**求值** `src/views/Setup.vue` 里两个 guidance 块的 `v-if`
表达式，往那些条件里引入新变量名会让它抛错。

## 排版

Rust 排版以 `rustfmt` 默认值为准（仓库不放 `rustfmt.toml`，所以编辑器与 `cargo fmt`
一致）。基线是干净的，因此在 worktree 里跑 `cargo fmt` 只会重写你自己刚写的那些行，不会
碰到别人的；`pnpm verify` 里包含 `cargo fmt --check`，提交前跑它即可。

前端排版以 Prettier 默认值为准：`prettier.config.mjs` 只有 `semi: false` 与
`singleQuote: true` 两条——树里本来就没有分号、只用单引号，照默认值排版等于改写每一行；
其余（缩进、换行宽度、多行列表尾逗号、单参数箭头函数的括号）一律用默认值，别再往里加
选项。`pnpm format` 就地排好。`.prettierignore` 只排除四类不该由它管的东西：Rust（归
rustfmt）、Markdown（Prettier 会重排正文段落）、YAML（工作流、pnpm workspace、lockfile）
与 Tauri 自己的 JSON。

## 本机自测 App

自测副本固定在 `/Applications/dsh-launcher.app`。`pnpm tauri build --bundles app`
只产出 target 里的 `.app`；`pnpm app:local` 一步构建 + 签名 + 替换 + 校验
（`--no-build` 只补签 + 替换；会先退出运行中的实例）。

**签名必须用证书，ad-hoc 不行**：ad-hoc 的 designated requirement（DR）就是 `cdhash`，
App 一重建，macOS 存的 TCC 授权（如「完全磁盘访问」）就失配——症状隐蔽，系统设置里开关
还开着、实际没权限。证书 DR 只钉 bundle id 和证书（不是 Team ID），重建后不变，所以
bundle id（`in.dsh-plug.dsh-launcher`）和安装路径别改，证书换发后重新授权一次即可。
完整说明见 `ci/build-local-app.sh` 头注释，脚本发现 DR 含 cdhash 会直接报错退出。

对着用户的真实配置跑 `pnpm tauri dev` 会读写
`~/Library/Application Support/in.dsh-plug.dsh-launcher/` 并对真实实例动手，所以真机验证
一律用无窗口 harness + 临时数据目录（手工点击验证是用户自己的事），细节见
`docs/testing-on-a-real-host.md`。

## 打包

仓库不出 tag、不发 Release：CI 只在 `main` 与 PR 上跑门禁，产物由自己在这台 Mac 上打。
`pnpm tauri build` 出 `.dmg`（未签名、未公证），`pnpm app:local` 出装到 `/Applications`
的自测副本（证书签名，见上一节）。版本号三处（`package.json`、`src-tauri/Cargo.toml`、
`src-tauri/tauri.conf.json`）由 `node ci/check-versions.mjs` 盯着保持一致，改就三处一起改。
