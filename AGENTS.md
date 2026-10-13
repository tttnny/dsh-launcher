# AGENTS.md — DSH Launcher

Tauri 2 + Vue 3 的 macOS Apple Silicon 桌面启动器：管理多版本 DSH 与多实例，
每个实例有独立的 `DSH_HOME`、Profile 和环境变量。运行数据在
`~/Library/Application Support/in.dsh-plug.dsh-launcher`（布局见 README「运行数据」）。

**打开实例 = 用系统浏览器打开实例 URL（契约，勿回退）**：经 `open that`，
与 `open_external` 同策略（先 trim 再只放行 http 与 https）。
`open_instance_window` 保留命令名和只传 id 的签名，后端从 running 表查 URL；
不存在任何实例 webview（回退史见 ADR-0002）。

**导航只有三个目的地：实例 / 资源库 / 设置（契约，见 ADR-0004）**：
`src/router/index.ts` 是唯一的路由表，`src/shortcuts.ts` 是快捷键与后端
`menu-navigate` 载荷的唯一映射。旧路径（`/homes`、`/profiles`、`/versions`、
`/tasks`）必须继续重定向；托盘与原生菜单的载荷指向 `/library?tab=…`。
`/plugins` **不在这份名单里**——它一直是插件页本身（只由运行中的实例进入），
别把它改成重定向。加第四项顶级导航前先读 ADR-0004。

**实例是一条绑定记录（契约，见 ADR-0003）**：`create_instance` 只绑定已安装的
版本，不建目录、不碰 DSH_HOME；`delete_instance` 只删记录（加上实例自己的图标
文件），版本与 DSH_HOME 一律留下。破坏性删除只属于「删除版本」。
「复制实例」是同一个命令加一个预填的弹窗，不要再加 `duplicate_instance`。

**插件管理属于运行中的实例（契约）**：入口只有实例侧两处（详情页头部按钮、实例行
展开卡片里的按钮），且只在实例 `running` 时出现；Profile 列表不放插件入口，只陈述
「哪些实例默认用它」。跳转带 `?instanceId=` 与 `?from=`（`instances` /
`instance-detail`，由 `usePluginLink.openPlugins` 写入），「返回实例」据此回到用户
离开的地方：从实例行走就是「实例页 + `?focus=<id>`」（那一行的卡片自动展开、滚动到
位并闪一下），从详情页走就是回详情页。别再把它钉死到某一个页面。

**插件页只管理补丁层（契约）**：插件页 = 补丁层（Bundle）列表，*条目（entry）不做
控制面*——`list_plugins` 只调 `listBundles`，每条层上报它带多少条目（`row_count` →
`BundleItem.rowCount`），前端不列条目标题、不给条目标开关，因此也没有搜索 / 全选 /
批量开关（条目是 200+ 量级，补丁层是 30 以内：控制面收在层上才对）。行内降噪规则
不变：`bundleHasOwnTitle` 比较的是「标题与包名是否不同」，不同才占第二行。

**插件页的写操作与 DSH 自带插件页同源（契约）**：开关 = 原生 `setBundleEnabled`，
卸载 = 原生 `removeBundle`——同一个方法、同一个权威（运行中的实例）、同一把文件锁，
文案也跟着 DSH 叫「卸载」（`removeBundle` 对 Profile 装的依赖真的会 `pnpm remove`，
只对 installation 自带的层才是纯取消选择；叫「摘掉」会让人以为依赖留下）。**不做**的
是原生页比我们多的那几件事：条目级开关（`setPluginEnabled`）、安装 / 更新
（`installBundle` + 进度 + 取消）、每个层自己注册的配置页、版本兼容性豁免
（`setVersionExemption`）。要加其中任何一项，先读 `packages/boot/plugin-manager`
的 `types.ts`：它们的返回形状与进度事件比开关复杂一个量级。

**插件页按「归属」分两组，状态不做分类（契约）**：列表只有「你添加的」（`removable`，
默认展开，行内可卸载）与「随 DSH 发布」（`removable:false`，默认折叠，只报个数）——
**不要**按 `enabled` 再切一个「未挂载」组：挂没挂载是状态，开关和状态点已经说了，而且
按状态分组的折叠会让刚被切掉的那一行从指针底下跑掉。两组的组头都是折叠开关并带计数，
所以折叠不等于隐藏。段标题报状态总数（「已挂载 N / M」）。写入返回
`restart-required` 时，页面顶部留一条「改动要重启才生效」的提示 + 重启按钮（实例
状态一变就撤掉），不要只靠一条 toast。

**未运行时给的是「文件投影」，不是实例的答案（契约）**：没有实例应答（没在跑 / 被
接管 / 连不上）时，页面用 `plugins::offline_bundles` 读 Profile 自己的 `package.json`
——它声明的层（`dsh.profile.bundles`，即优先级顺序）在先，装了的依赖在后，
`version` 从 `node_modules/<name>/package.json` 读。这份投影**只读**（ADR-0001 一条都
不破），而且**故意比在线列表薄**：没有实时状态、可用性、卸载规则、条目数，`removable`
一律 false——所以它在界面上也长得不一样：平铺、不分组（分组依据是 `removable`，文件
证明不了），开关禁用，脚注说明它读自文件。别把它当成实例的列表，也别据此把层归到
「随 DSH 发布」那一组。

**写入时锁住整页（契约）**：实例是拿一把 profile 文件锁串行写盘的，第二个点击会排在
第一个后面、还可能回一个看不懂的拒绝。所以只要有写入在飞（`bundlePending` 非空），
页面上所有开关与卸载按钮一起禁用（`BundleRow.locked`），跟原生插件页的行为一致。

**启动 token 要跨会话记住（契约）**：实例就绪时打印的 URL（含 `token=`）写进
`<data_dir>/running.json`（`process::remember_url`），实例停止 / 退出时删掉。收养
外部进程前先用它验证（`native::NativeClient::connect` 成功才沿用，失败即丢弃）——
否则每次「启动器更新或退出重开、实例还在跑」都会白丢 token，插件页从此只剩一句
「由系统接管」的死路。因此「能不能管理插件」只由 `process::manageable_url`（URL 里
有没有 token）决定，**不要**再把 `adopted` 当拒绝理由：`adopted` 只表示「停止时按
pid 杀」。只读横幅必须带一个能离开该状态的按钮（启动 / 重启实例）。

**启动时收养幸存进程（契约）**：`process::adopt_surviving_instances` 在 app 启动后
后台跑一遍——被替换 / 强杀留下的 DSH 进程会一直占着端口，不主动认领的话，界面会一直
说「已停止」（插件页退化成文件列表），直到用户碰巧点一次启动。认领依据：用户钉死的
端口只要有 DSH 样进程即可；随机端口必须先用 `running.json` 里的 token 认证成功才认，
否则那个端口可能是别的程序。

**DSH 的展示文案是「字符串或语言映射」（契约）**：`meta.title` / `meta.description`
对官方插件是普通字符串，对本地化过的插件是 `{"en": …, "zh": …}`。凡是有可能拿到
翻译的字段一律走 `native::NativeText`（`pick` 按 UI 语言取：精确 tag → 基础语言 →
`en` → 任意），**不要**声明成 `String`——一个翻译过的标题就会让整张列表解析失败
（症状：插件页弹「解析插件列表失败: invalid type: map, expected a string」）。
投影给前端的 `BundleItem` 里只有解析后的单语字符串，mock 也按这个投影后的形状写。

**实例行没有溢出菜单（契约）**：行本身只有「这台实例是什么」+ 选 Profile + 主操作 +
行尾一个只作指示的展开箭头（点行或点实例名都展开，不是跳转）。实例的动作全部在展开
卡片里，按「作用对象」分三段、排成两行：第一行是进程态（重启 / 停止 / 管理插件，仅
运行或启动中）│ 机器（终端 / 目录 / 日志），第二行是绑定（实例设置 / 复制实例 /
重新读取 Profile）。每段各自成行、段内才换行（`.facts-actions` 竖排 + `.facts-line`
横排）：都挤在一行时尾部的按钮会被宽度顶成孤立的一行（实例设置、复制实例都中过招）。
不要再引入 `⋯` 之类的 hover 菜单，也不要往行上塞图标按钮。

**`--preserve-symlinks` 不做用户开关（契约）**：它由 profile 清单推导
（`crate::profile::preserve_symlinks_for` → `declares_link_dependency`），实例启动、
实例终端、模板引导三处同源。重新引入 per-instance 开关前先读 `launch.rs` 里那段
注释：这个 flag 的错误方向是「DSH 设置写入全失败」，不能让用户手动开错。

**颜色只表示状态**：`src/styles/tokens.scss` 是唯一的调色板来源，机器事实
（版本号、端口、URL、路径、条目 id）一律加 `.dl-mono`。不要在视图里再写一份
按钮/卡片样式——`.dl-btn`、`.dl-surface`、`.dl-chip`、`.dl-status` 在
`src/styles/index.scss` 里定义一次（搜索框是 `.dl-input.dl-search`，实例列表与版本
列表共用）。

## 日常开发

- 验证走 `pnpm tauri dev`（真桌面 + Rust 后端）；`pnpm dev` 只有浏览器
  localStorage mock。
- 新增 Tauri 命令要同时改三处，否则 `ci/api-contract.test.mjs` 会失败：
  `src/api/index.ts`（interface）、`src/api/mock.ts`（switch case）、
  `src-tauri/src/lib.rs`（`generate_handler!`）。
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
Latest，也不抬版本号；`bump-version` job 只在**推 tag 且是正式版**时跑（工作流里的
条件就是 `startsWith(refs/tags/v) && prerelease == false`），它把 manifest 抬到下一个
patch 并推回 main。

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
