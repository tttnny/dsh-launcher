# 版本、DSH_HOME 与插件

管「一个实例由什么组成」的三样东西——版本、DSH_HOME、Profile——以及挂在 Profile 上的
插件补丁层。它们都归某台主机的文件系统（术语见 `GLOSSARY.md`），所以装、删、读写都要在
**那台主机**上发生；这一份讲本机与远端的同一套规矩，主机记录与传输层见 `hosts.md`。

## DSH 的展示文案是「字符串或语言映射」

症状先记在这里，省得以后当成「解析代码写错了」查半天：`meta.title` / `meta.description`
对官方插件是普通字符串，对本地化过的插件是 `{"en": …, "zh": …}`。凡是有可能拿到翻译的字段一律走 `native::NativeText`，
**不要**声明成 `String`；一个翻译过的标题就会让整张列表解析失败，插件页弹「解析插件
列表失败: invalid type: map, expected a string」。投影给前端的 `BundleItem` 里只有解析
后的单语字符串，mock 也按这个形状写。

## 全新的 DSH_HOME 先在那儿跑一次 DSH

`profiles/web` 是 DSH 第一次在一个 HOME 里运行时才建出来的，所以两个建实例的入口——安装
任务（`do_create_instance`）与「新建实例」（`commands::create_instance_in`）——都要补上
这一步：本机 `tasks::ensure_web_profile_template`，远端
`process::ensure_remote_web_profile_template` 在**那台机器上**做同一件事。少了它，那个
HOME 下一个 Profile 都选不出来，「启动」点了不会动。**判据是「那个 HOME 下一个 Profile
都没有」**，不是「这是启动器建的 HOME」：已经有 Profile 的 HOME 一个字节都不碰。

## 删版本要连专属 DSH_HOME 的记录一起删

`remove_version` 用同一条路径规则（`<数据目录>/homes/<版本名>`）删两样东西——目录与配置
里的那条记录，两者在同一个状态保存里完成。删完版本前端要一并刷新 DSH_HOME 列表
（`store.refreshHomes()`），浏览器 mock 的 `remove_version` 也照同一条路径规则删。
**比较两个路径问主机**：`Host::paths_equal` 是本机 / 远程唯一的入口。

## 装版本与删版本都发生在那一台主机上

`start_install_version_task` 带主机，安装用那台主机的数据目录、registry、额外环境变量与
探到的**绝对**工具路径，脚本里不出现本机路径；进度与日志读的是那台机器上的日志文件。
取消要真的停掉那边的**进程组**——`stop_pid` 只停组长，会在服务器上留下占着 store 锁的
子进程，所以用 `stop_group`，而且要等那台机器确认进程真的没了才算「已取消」。点「取消」
与跑安装的 worker 会互相错过（点下去的那一刻正好没有命令在跑），所以已取消的任务不再起
新命令，worker 自己发现取消时也要停掉它刚起的那一个——两边都停，才不会留下占着 store 的
孤儿。删版本时远端目录与专属 DSH_HOME 都由那台机器删，且只删那台主机数据目录之内的路径。
**凡是按版本索引的东西都要连主机一起索引**（前端 `store.installingVersions` 用
`installKey(hostId, version)`）：两台机器可以同时装同一个版本，只按版本索引会让它们的
进度互相覆盖。

## 插件页只管理补丁层

页面做的就是三件事——列层、开关、卸载，都走 DSH 原生方法（`setBundleEnabled` /
`removeBundle`，同一个权威、同一把文件锁），文案也跟着 DSH 叫「卸载」。**条目
（entry）不做控制面**：`list_plugins` 只调 `listBundles`，层只上报它带多少条目，前端不列
条目标题、不给条目标开关，因此也没有搜索 / 全选 / 批量开关。原生页比我们多的那几件事
——条目级开关（`setPluginEnabled`）、安装 / 更新（`installBundle` + 进度 + 取消）、每个
层自己注册的配置页、版本兼容性豁免（`setVersionExemption`）——要加其中任何一项，先读
`packages/boot/plugin-manager` 的 `types.ts`：它们的返回形状与进度事件比开关复杂一个
量级。

## 未运行时给的是「文件投影」，不是实例的答案

没有实例应答（没在跑 / 被接管 / 连不上）时，页面用 `plugins::offline_bundles_in` 读
Profile 自己的 `package.json` 与各包在 `node_modules/<name>/package.json` 里的版本，远端经
`remote::profile_packages` 一次 ssh 读回。**这份投影读的是「那台机器上的」文件**，机器
本身连不上时**不读**——页面给的是「主机不可达」的原因与「重试」，而不是一份假列表。它是
**只读**的，而且**故意比在线列表薄**，所以界面也长得不一样：平铺、不分组、开关禁用、
脚注说明它读自文件。别把它当成实例的列表，也别据此把层归到「随 DSH 发布」那一组。
`plugins::offline_bundles` 是 Rust 内部函数，**不是 Tauri 命令**，前端不要调它，mock 也
不要实现它。

## 写入时锁住整页

实例是拿一把 profile 文件锁串行写盘的，第二个点击会排在第一个后面、还可能回一个看不懂的
拒绝。所以只要有写入在飞（`bundlePending` 非空），页面上所有开关与卸载按钮一起禁用
（`BundleRow.locked`），跟原生插件页的行为一致。

## 不给 dsh 传 Node 运行时 flag

`--preserve-symlinks` 已删除。它让 Node 把同一个包经多条路径加载成多个模块实例，而 DSH
的模块级状态按身份认人（工具调度器是模块级 `Symbol`，`dsh-app-boot` 是模块级 WeakMap），
于是整个进程坏掉：所有工具调用报 `Cannot read properties of undefined (reading
'prepare')`，设置写入也全被拒。代价是 `link:` 插件要靠自己 checkout 里的 `node_modules`。
要重新引入任何 node 运行时 flag，先确认 DSH 扛得住重复的模块实例。
