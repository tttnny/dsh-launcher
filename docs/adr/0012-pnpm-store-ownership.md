# 装版本与 Profile 共用一个 pnpm store，但 Profile 沿用已链接的那个

## 背景

pnpm 的 store 是内容寻址的：一个 `node_modules` 链接自哪个 store，之后对它做
add / remove 就必须用同一个 store，否则 pnpm 直接以 `ERR_PNPM_UNEXPECTED_STORE` 拒绝。
启动器自己起 pnpm 时能传 `--store-dir`，本机与远端都指向那台主机数据目录里的
`.pnpm-store`；但运行中实例的插件管理器是自己起裸 `pnpm` 的，看不到这个参数，退回
用户的全局 store，于是插件页里每一次装、卸都失败。

## 决策

**装版本、源码构建与 Profile 共用一套 store 规则：默认用那台主机数据目录里的共享 store，
Profile 优先沿用它已经链接的那一个。**

1. 共享 store 是 `<数据目录>/.pnpm-store`（`toolchain::store_dir`），每台主机一份；装
   版本、源码构建、插件安装都用它。
2. Profile 钉哪个 store 以磁盘上已有的链接为准：读 `node_modules/.modules.yaml` 的
   `storeDir`（旧 pnpm 写 YAML、pnpm 10+ 写 JSON，两种都读）。有链接就钉链接指向的那个
   （`plugins::profile_store_target`）；没链接——这个 Profile 还没装过东西——才钉启动器
   store。
3. 与启动器 store 互为祖先或后代时归成启动器 store 的基础目录：pnpm 记录的是
   `<base>/v<major>`，启动器钉的是 base。
4. 首次创建 `pnpm-workspace.yaml` 时写 DSH 初始化会写的那份默认值（`packages: [.]`、
   `nodeLinker: hoisted`、`autoInstallPeers: false`），而不是只加一行 `storeDir`。
   理由：DSH 的 profile 初始化从不覆盖已存在的文件，这里写薄了，这个 Profile 就永远拿不到
   `hoisted`，而树外的插件依赖它。
5. 远端先在拥有 Profile 的那台机器上读链接，再钉同一个 store；一次启动一次往返，不照本机
   路径写。

## 后果

- 用户手工用全局 store 装的 Profile 不会被拖到启动器 store 上：钉它原来的那个，只是把它
  写进文件，让实例里那个裸 pnpm 也知道。
- 启动器自己传的 `--store-dir` 仍然优先于文件里的值，所以启动器驱动的安装行为不变。
- 一个 Profile 用哪个 store 由磁盘上已有的链接决定，不由设置页决定；要换 store，得先重装
  它的依赖。
- 清理 pnpm 缓存时删的只能是启动器那个 `.pnpm-store`（纯缓存）；Profile 钉的用户 store
  是用户的东西。

## 备选方案

- **一律钉启动器 store**：被否。等于把不匹配换了个地方——用户手工装的依赖链接全部失效。
- **不钉，让插件管理器的 pnpm 用全局 store**：被否，正是 `ERR_PNPM_UNEXPECTED_STORE` 的
  来源。
- **每个 Profile 一个 store**：被否。白占磁盘、丢掉内容寻址的收益，还解决不了「两个 pnpm
  要一致」这件事。
