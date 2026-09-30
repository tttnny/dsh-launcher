# 网页端插件管理靠 profile 内的 storeDir 固定，而不是给实例进程设环境变量

## 背景

启动器把 DSH 的 pnpm 内容 store 钉在数据目录（`<数据目录>/.pnpm-store`），做法是
给它自己 spawn 的每一次 pnpm 传 `--store-dir`（`toolchain::pnpm_store_flags`）。
于是由启动器装出来的 profile，其 `node_modules` 链接自这个 store，
`node_modules/.modules.yaml` 里记的 `storeDir` 也是它。

网页端「插件」页的安装/卸载**不经过**启动器的 pnpm：它在实例自己的进程里，由
DSH 的 plugin-manager 服务在 profile 目录中跑裸 `pnpm`（`runProfilePnpm`），
压根看不到那个 flag。裸 `pnpm` 便回落到用户的全局 store（本机
`~/Library/pnpm/store/v11`），而 pnpm 一旦发现 `node_modules` 链接自另一个
store，就对**任何**增删直接失败：

```
[ERR_PNPM_UNEXPECTED_STORE] Unexpected store location
The dependencies at ".../profiles/web/node_modules" are currently linked from the
store at ".../in.dsh-plug.dsh-launcher/.pnpm-store/v11".
pnpm now wants to use the store at "/Users/tny/Library/pnpm/store/v11" ...
```

报错码被 DSH 的失败分类器归为项目级错误（`classifyInstallFailure`），
浏览器错误串里看不到 `ERR_PNPM_` 前缀，只剩 pnpm 自己的正文——这解释了为什么
用户看到的两条报错一条带码、一条只有正文。

启动器原有的对策只在它自己的 CLI 路径上（`run_dsh_plugin` 前置检测
`.modules.yaml`，不一致就先重链接），对实例内进程无效：那条路径根本不会被走到。

## 决策

**把 store 固定写进 profile 的 `pnpm-workspace.yaml`。**

pnpm ≥10 从 `pnpm-workspace.yaml`（不是 `.npmrc`）读 workspace 级设置，其中
`storeDir` 正是 pnpm 记录的同一个键名。启动实例前（以及每次
`dsh plugin` 调用前）确认该文件里有 `storeDir:`，值为该 profile 应当使用的
store，于是任何在 profile 目录里跑的 pnpm——包括启动器看不见的那个——都自动
落到同一个 store。

选值规则（`plugins::profile_store_target`）：**已经装过的 profile 钉它自己链接
的 store**，只有从未装过的才钉启动器的共享 store。反过来（无条件钉启动器
store）会把用户手工对着全局 store 装好的 profile 硬搬到启动器 store 上，只是把
不一致换了个方向——搬完 `node_modules` 仍链在旧 store，错误照旧。

**为什么不是给实例进程设 `pnpm_config_store_dir`。** 环境变量会被实例进程下的
**所有**子进程继承：用户在该实例的终端里手跑一次 `pnpm install`，会莫名其妙
落到启动器的 store。`dsh-subprocess` 的 `scrubbedParentEnv()` 只过滤密钥形状与
`DSH_*` 变量，不会挡它——它是「这个 profile 的事实」，不是「这台机器的偏好」，
写进 profile 才是它该在的层级。

**为什么不是 `.npmrc`。** pnpm ≥10 不再从 `.npmrc` 读这些设置；实测
`store-dir=` / `store_dir=` / 带引号 / 裸空格路径全部被忽略，仍落到全局 store。

**兼容性。** 显式 `--store-dir` 优先于文件里的 `storeDir`（pnpm 11 与 12 实测
一致），所以启动器自己的调用行为不变；pnpm 11 与 12 都认这条设置。写入用双引号
包住值——数据目录路径含空格（`.../Application Support/...`），裸写会被 YAML 从
空格处截断。文件不存在时按 DSH `initProfile` 的同一份模板落盘（含
`nodeLinker: hoisted`）：那个写入方从不覆盖已存在的文件，先落一份更薄的会让
profile 永久失去该设置。

## 后果

- 网页端「插件」页的安装/卸载在启动器起的实例里可用，不再要求用户去终端
  `pnpm config set store-dir`。
- profile 多出一条启动器维护的设置行。它是该 profile 的使用事实，profile 被复制/
  导出时会跟着走；导入到另一台机器上路径失效时，用户的全局 pnpm 配置会重新
  生效（显式配置优先于这条），行为与导入前一致——但下一处启动器写入会再按该机
  器的情况修正。
- 启动器自己 spawn 的 pnpm 仍需 `--store-dir`：它要覆盖的正是 profile 里这条
  记录（比如用户手工改过），两处一致才不会有意外。
- 依旧保留 `run_dsh_plugin` 的重链接兜底：历史 profile 的 `node_modules` 可能链
  在别处，重链接与固定是同一件事的两步。

## 备选方案

- **给实例进程设 `pnpm_config_store_dir`**：被否，会继承进用户终端里的手跑 pnpm，
  把「这个 profile 的事」扩大成「这个进程树的事」。
- **改 DSH 源码，让 plugin-manager 转发启动器的 store**：被否，启动器要能驱动
  用户自行安装的任意 DSH 版本，改上游不解决问题。
- **写 `.npmrc`**：被否，pnpm ≥10 不读它（实测四种写法全部无效）。
- **无条件钉启动器共享 store**：被否，会与已经手工装好的 profile 冲突，见上。
- **只在实例启动时写、不在 `dsh plugin` 路径写**：被否，两条路径都会留下需要
  固定的 profile，漏一条就还有复发的入口。
