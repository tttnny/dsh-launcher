# AGENTS.md — DSH Launcher

Tauri 2 + Vue 3 的 macOS Apple Silicon 桌面启动器：管理多版本 DSH 与多实例，
每个实例有独立的 `DSH_HOME`、Profile 和环境变量。运行数据在
`~/Library/Application Support/in.dsh-plug.dsh-launcher`（布局见 README「运行数据」）。

**规则在 `docs/agents/rules/` 里，按主题分成五份**。凡是要改动一个行为、改一条数据
的归属、加一个命令或一个状态值，先读相关的那一份：里面写的是只看代码学不到的东西
——约定、取舍的理由、踩过的坑，以及各处不可回退的契约。代码自己说得清的事那里不写。

| 文档 | 管什么 |
| --- | --- |
| `docs/agents/rules/instances.md` | 实例这条记录是什么、进程状态怎么变、实例行怎么排 |
| `docs/agents/rules/versions-and-homes.md` | 版本、DSH_HOME、Profile、插件，归谁、在哪儿装与删 |
| `docs/agents/rules/frontend.md` | 四个页面的导航与跳转、状态怎么显示、样式写在哪里 |
| `docs/agents/rules/hosts.md` | 本机以外的机器：传输层、隧道、远端起停、环境检测 |
| `docs/agents/rules/dev.md` | 门禁、测试、排版、真机验证、自测 App、打包 |

读的时候手边放上 `GLOSSARY.md`（词义按它，不要另造说法）与 `docs/adr/`（每条契约的
完整推理）。真机验证与无窗口 harness 见 `docs/testing-on-a-real-host.md`。

## 日常开发

- 提交前跑 `pnpm verify`；想快过一遍用 `pnpm verify:quick`；想单独跑一个门禁用对应的
  `pnpm test:*` 脚本。
- 手工点击验证走 `pnpm tauri dev`（真桌面 + Rust 后端）；`pnpm dev` 只有浏览器
  localStorage mock，测不到真实行为；装到 `/Applications` 的本机副本用 `pnpm app:local`。
- 新增 Tauri 命令要同时改三处，参数拼写两处必须一致。
- 其余门禁、测试写法、排版、打包与自测 App 的细节全在 `docs/agents/rules/dev.md`。

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
