# 接受下限以上的 pnpm，下载配置走环境变量，构建批准只认 pnpm 报出的包名

## 背景

启动器装版本与源码构建都用用户自己的 pnpm。原先把接受的版本钉死在主版本 11，理由是
「DSH 的 profile 由 pnpm 11 初始化」——这条前提不成立：DSH 初始化 profile 只写
`package.json`、`cordis.patch.yml` 与 `pnpm-workspace.yaml`，任何地方都不校验 pnpm
版本。而 brew、`npm i -g`、官方脚本这几条渠道现在都给 12，钉死主版本等于拒绝大多数
用户已经装好的工具链。另两件事必须一起解决：pnpm 10 起默认不跑依赖的构建脚本，原生模块
不编译、DSH 启动只报一个看不出原因的 `MODULE_NOT_FOUND`；下载健壮性参数在不同主版本上
写法不同。

## 决策

**接受已验证下限以上的 pnpm，用环境变量传下载配置，按 pnpm 报出的包名写构建批准。**

1. **接受已验证下限以上的 pnpm，不锁主版本。** 下限是 11（`MIN_PNPM_MAJOR`），启动器的
   旗标集与 store 布局只在 11 及以后验证过；连版本号都报不出来的 `--version` 不接受。
2. **下载配置走 `pnpm_config_*` 环境变量，不走 `--config.*` 旗标。** 旗标形式看着跨
   版本，在 pnpm 11 上却是陷阱：它用空类型表重解析 `--config.` 参数，`fetch-timeout`
   以字符串身份到达 `AbortSignal.timeout()`，抛出的 `ERR_INVALID_ARG_TYPE` 被吞掉，
   安装的 promise 永不结算——任务卡在接近 100% 的进度上，一个字节都没下载。环境变量经
   配置读取器按声明类型转换，两个主版本都拿到数字。`--network-concurrency` 没有跨版本
   写法，故意不传，用各版本默认值。
3. **构建批准写在版本目录的 `pnpm-workspace.yaml` 里。** pnpm ≤10 只认
   `onlyBuiltDependencies: ['*']`；11 与 12 认 `allowBuilds`，而这两个新主版本又不一样：
   pnpm 11 把被跳过的依赖以 `name: set this to true or false` 的占位符写进清单，下一轮
   把占位符改成 `true` 即可；pnpm 12 什么都不写、只在输出里报
   `Ignored build scripts: name@ver, …`，且不接受通配符——**它报出的包名是唯一的批准
   依据**，没报过的名字不能猜（带域的包名按 YAML 键加引号，版本号不算数）。
4. **有限轮次。** 一轮用来批准 pnpm 报出的名字，一轮用来改写 pnpm 自己写下的占位符；同样
   一份拒绝在第二轮之后回来，就是启动器不认识的机制，结束安装并说清试过什么、还没成的
   原因。判断只读本轮输出——任务日志里还留着上一轮已经回答过的拒绝。
5. **装完还要复查。** `node_modules/.modules.yaml` 里仍有 `pendingBuilds` 时跑一次
   `pnpm rebuild` 补建；补不齐就拒绝登记这个版本，半成品树启动只会得到一个莫名的
   `MODULE_NOT_FOUND`。
6. **本机与远端共用一份实现。** 清单由 `plugins::build_scripts_manifest` 生成、轮次由
   `tasks::BuildApprovals` 管；远端只是把同一份文档写到那台机器上。源码构建不走这套
   ——检出目录自带上游清单与构建策略，只是装完仍复查 `pendingBuilds`。

## 后果

- 任何装版本的路径都必须先写这份清单，漏写的树跑不起来。
- 新增一种 pnpm 主版本时要核对两件事：批准字段怎么写、下载配置怎么传；不认识的批准机制
  会以一次明确的失败结束，而不是永远转圈。

## 备选方案

- **钉死 pnpm 11**：被否。拒绝用户已经装好的工具链，而收益是零——DSH 不校验版本。
- **继续用 `--config.fetch-*`**：被否，在 pnpm 11 上直接挂死。

## 追加（2026-11 注释清理）

- 构建批准清单里先写 packages: 根键：pnpm 按它把这个目录当工作区，allowBuilds 才有地方被读到；文件为空时先写 packages: [.]，再写批准项。
- 本机与远端都在没有 TTY 的环境里跑 pnpm，必须带 CI=true：pnpm 清理 node_modules（store relink）时要确认，没有终端就以 ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY 中止，安装看起来像无缘无故失败。
