# DSH 的三种调用形态，与每个参数属于谁

## 背景

启动器拉起 DSH 的地方有三处：跑实例（本机或远端）、起一次临时进程生成 Profile 模板、给实例开终端。
三处各自拼命令行就会分叉；而 `--host` 会盖掉 Profile 层的绑定决定，Node 解释器按名字解析又取决于启动器继承到的环境——两件都一错就难查。

## 决策

**DSH 的命令行只由 `launch` 模块构造，三种形态参数固定，实例形态绝不带 `--host`。**

1. **实例**：`<绝对路径的 node> <版本>/…/bin.js --profile P [--port N] [--no-open]`
   （`instance_command`；远端把同一条 argv 交给传输层，`instance_argv`）。`--host` 永远不
   传：127.0.0.1 本来就是 webserver 行的默认，而显式的 host 会压过 profile 层（remote-web-ui
   插件的局域网开关），传了「局域网访问」就永远开不了。`--port` 只给 web profile（0 = 随机），
   `--no-open` 只在那个版本的 bundle 懂这个旗标时才传。
2. **模板**：`--profile web --host 127.0.0.1 --port N`（`template_boot_command` /
   `template_boot_argv`）——唯一允许带 `--host` 的调用，因为它是一次性生成 Profile 的短命进程，
   绑哪里是启动器自己的决定：本机给 20000 起的随机端口，远端给 0 由那台机器的内核分配。
3. **终端**：不跑启动器的 argv。把版本的 `node_modules/.bin` 放进 PATH 最前面，并在本机写一个
   `#!/bin/sh` 起头、`exec node <bin.js> "$@"` 的包装器（`ensure_terminal_dsh_wrapper`），让手敲
   的 `dsh` 与启动器拉起的是同一个 CLI；环境由 `terminal_env_pairs` 导出（DSH_HOME、实例标记与
   实例变量，DSH_HOME、PATH 保留）。远端不写文件，只把那边版本的 `.bin` 放到 PATH 前。

跨形态还有三条：

- **Node 用绝对路径**（`runtime::node_for_spawn_checked*` 解析出的绑定），不靠 PATH：从 Finder
  启动的应用只有最小 PATH，靠名字解析会把实例绑在「启动器碰巧继承到的环境」上。
- **入口就绪只有一句判定**（`launch::version_bin_ready`）：装版本后的核验、启动前的检查、模板
  生成都问它；pnpm 装完的链接可能短暂滞后，所以缺失或空文件时每 150 毫秒复查一次、最多五次。
  最后那个分支只回答「路径存在」，调用方不能当「可读、可执行」用。
- **代理的作用范围**：启用后，启动器自己的 HTTP 请求（GitHub 标签列表、图标抓取、更新检查与
  安装包下载）与版本安装执行的命令（`npm view`、`pnpm install`、`git clone`、`npm install -g`）共用
  同一份配置（`proxy::apply` / `proxy::apply_to_command`）；实例的环境注入另由一个开关控制
  （`proxy_apply_dsh`）；远端实例不带本机代理——代理描述本机的网络出口，远端出口由主机记录的
  额外环境变量决定。对实例本身的 HTTP（插件页、token 换取）不走代理：那一定是回环地址。

## 后果

- 本机与远端共用同一份 argv 生成；加参数时先想清楚它属于哪种形态：`--host` 只属于模板，
  `--port` 只属于 web profile。
- 包装器只改启动器自己写的那一份（`#!/bin/sh` 加 `exec node`），pnpm 提供的 `dsh` 不动；远端没有包装器。

## 备选方案

- **每个调用点各自拼 argv**：被否。本机、远端、终端三处的参数会分叉，改一处漏两处。
- **所有形态统一传 `--host 127.0.0.1`**：被否。Profile 层的局域网开关会失效。
- **给实例加 `--preserve-symlinks` 支持 `link:` 插件**：被否。同一个核心模块经不同路径进来会有
  多个身份，DSH 用模块级 Symbol / WeakMap 存状态，重复实例会让工具调用整体失效（`link:` 插件靠
  kernel-package 路由解决）。
