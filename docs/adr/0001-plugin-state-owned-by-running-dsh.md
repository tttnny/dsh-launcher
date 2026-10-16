# 插件状态的权威是运行中的 DSH，不是启动器

## 背景

启动器曾经自己实现了一套插件管理：直接用行扫描改 `cordis.patch.yml` 来启停插件，
用 `dsh plugin --profile <p> remove` 来卸载。两处都跟 DSH 的原生实现不一致，其中
前一处会**写坏文件**——删「匹配 id 的行」时只删了 `- id: <x>` 那一行，它的
`name:` / `config:` 子行留下来挂到上一条，生成重复键，profile 从此加载不了。

而 DSH 的插件管理本来就是它进程内的一个服务，网页端通过
`POST /api/pluginManager/<方法>` 调它。DSH 自家的 Electron 桌面端也是独立进程，
同样是纯 HTTP 调这个服务（`fetch(带 token 的 URL)` 取 `set-cookie`）。

## 决策

**启动器不再自己写任何 profile 文件。** 插件的列表、启停、卸载全部走实例的
`/api/pluginManager/*`，凭据用 running 表里那个带 token 的 URL 现换 Cookie。

这是「对齐原生」的字面实现，也删掉了唯一一处会写坏 profile 的代码。

代价：**插件操作要求实例正在运行**。没跑时插件页只读，并提示去启动实例。

## 后果

- `profile.rs` 的 `set_disabled_row` / `strip_cordis_rows`、`plugins.rs` 的
  `resolve_home_paths` / `run_dsh_plugin` 整段退出。`dsh plugin` 那条 CLI 路径
  不再被插件管理使用。
- 启动器挑「最新版本 CLI」的隐患随之消失——不再需要挑。
- Cookie 绑在 `host:port` 上，每个实例每次启动都要重新换一次，不能跨端口复用。
- 请求信封是严格校验的，得照抄：
  `{"type":"client-request","rpcId":"…","method":"pluginManager/setBundleEnabled","payload":{"args":{…}}}`。
- 启动器的控制面停在**补丁层（Bundle）**：挂载 / 停用 / 卸载。层里的插件条目由实例
  自己报个数（`BundleItem.rowCount`），页面不再逐条开关——条目是 200+ 量级，逐条开关
  既没有意义也放不下，那是 DSH 自己界面的活。
- 卸载时原生**不会**清理 `cordis.patch.yml` 里引用被删插件的行。那些行是**惰性的**
  ——bundle 一摘，引用它的行就不再参与组合。启动器**故意不补这一步**：一补就又把
  「启动器也写 profile 文件」这个口子打开了，而那正是本 bug 的成因。
- 测试要覆盖「实例未运行」这条路径——它是新的常态分支，不是异常。
- 「没在跑」时页面给的是**文件投影**，不是实例的答案：读 Profile 自己的 `package.json`
  （`dsh.profile.bundles` 就是优先级顺序）与 `node_modules/<name>/package.json` 里的版本。
  远端经 `remote::profile_packages` 一次 ssh 把 `package.json` 与各包版本一起读回来——
  一个包一次连接会让「没有实例时」比有实例时还慢——本机与远端共用同一份纯投影
  （`plugins::offline_bundles_from`）。机器连不上时**不读**：那路径不在本机，本机的 `fs`
  只会问到这台 Mac 上同名的地方，页面给的是「主机不可达」与「重试」，而不是一份假列表。
- 这份投影**故意比在线列表薄**：没有实时状态、可用性、卸载规则与条目数，`removable`
  一律 false。界面据此与在线列表长得不一样——平铺、不分组（分组依据正是 `removable`，
  文件证明不了），开关禁用，脚注说明它读自文件。别把它当成实例的列表，也别据此把层归
  到「随 DSH 发布」那一组。
- `plugins::offline_bundles` 是模块内的函数，不是 Tauri 命令：前端不经它，mock 也不实现它。
- 行内降噪：`bundleHasOwnTitle` 比的是「标题与包名是否不同」，相同就不占第二行。

## 备选方案

- **离线写，但照抄原生规则写对**：被否。`writePluginEnabled` 没有从包里导出
  （`plugin-manager/package.json` 的 `exports` 不含它），只能重写一份；模仿会在
  上游改动后再次失配，正是这个 bug 的成因。
- **混合（跑着调原生，没跑离线写）**：被否。同一份状态两个权威，等于把上面的
  风险留下，还要多维护一套。
- **修好那段行扫描**：治标。文件格式的规则属于 DSH，不属于启动器。
