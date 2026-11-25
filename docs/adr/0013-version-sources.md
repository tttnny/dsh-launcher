# 可安装版本以 npm registry 为主，GitHub-only 的 dsh-v* 标签从源码装

## 背景

可安装的版本有两条来源：npm 上发布的 `@deepseek-ai/dsh`，以及上游只在 GitHub 打
`dsh-v*` 标签、从没发到 npm 的 alpha。只查一边，列表要么不全，要么列出装不上的东西。
还有一处归属问题：远端主机的 registry 可能是只有那台机器够得着的镜像，在本机取列表就会
给出在那里装不了的版本，而装的动作本身又发生在那台机器上。

## 决策

**列表以 npm 为主，按主机决定问哪台机器的 npm，GitHub-only 的标签并入并从源码装。**

1. **npm 优先。** 查 `@deepseek-ai/dsh` 的 `versions` 与 `time`（发布时间），列表带发布
   时间。
2. **registry 归属「查询发生的那台机器」。** 本机问本机 npm（主机记录里的 registry 优先，
   其次 `DSH_NPM_REGISTRY` 镜像）；远程主机配了自己的 registry 就在那台机器上问（一次
   ssh），没配就在本机问——那就是启动器本来就在查的源，不额外起一次 ssh、不多一种失败。
3. **GitHub-only 的标签并入。** 再查上游 releases 的 `dsh-v*` 标签，标 `source: github`
   后并入；npm 已经有的版本不重复。标签先过版本串校验——Git 标签允许 `/` 与 `..`，
   直接当目录名会跑出数据目录。列举失败只记日志，不影响 npm 那份列表。
4. **装的时候按那台机器自己的 npm 分流。** 那台机器的 npm 有这个版本就 `pnpm install`；
   没有就先确认上游有 `dsh-v<版本>` 标签，再从源码装：浅克隆该标签到版本目录 →
   `pnpm install --frozen-lockfile` → `pnpm run build`。两边都没有就报错，不猜。
5. **列表与安装必须用同一个 registry。** 列表用哪个源，安装就用哪个源，否则界面上的版本
   与装下来的版本来自两个不同的世界。
6. **两种版本树布局并存，启动器按磁盘认。** npm 树：`node_modules/@deepseek-ai/dsh/lib/bin.js`；
   源码检出：`apps/cli/lib/bin.js`（判据是该目录里有 `apps/cli/package.json`）。web 应用
   的启动脚本同理：npm 树读 `.pnpm` 下那份规范副本，源码检出读
   `packages/bundle/web-app/`。两种布局由同一份 `launch::version_bin` 推导入口；源码装
   构建完确认入口存在才登记，npm 装的树在启动时同样被这一步拦下。

## 后果

- 源码版慢得多：一棵完整的 monorepo 依赖 + 一次全量构建。换来的是 alpha 可用，而客户端、
  启动与 PATH 登记都按版本树认，不区分来源。
- GitHub 列举失败时列表变短：用户看到的是「没有这个版本」，而不是「问不到 GitHub」。
  日志里有原因。
- 远端没配自己的 registry 时列表在本机取，可能不含那台机器才够得着的包。这是有意的取舍，
  配上 registry 就解决。
- 列表按主机缓存（`refreshRemoteVersions`）；registry、ssh 目标或数据目录变了就清掉缓存。

## 备选方案

- **只查 npm**：被否，alpha 版全部不可见。
- **一律在本机查**：被否。远端专用镜像上的版本会出现在列表里，点下去却在那台机器上失败。
- **一律在那台远端机器上查**：被否。没配专用源的主机也得靠 ssh 才能看到通用版本。
- **只支持一种版本树布局**：被否，那等于没有源码安装这条路。
