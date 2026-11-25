# macOS 上开终端与访达走 osascript，不走默认打开方式

## 背景

启动器有两处「交给系统」的动作不是「按默认方式打开」能满足的：给实例开一个终端窗口（在新
窗口里跑一条命令），和在访达里打开一个目录（最好复用已经开着那个目录的选项卡，而不是攒一
屏窗口）。Tauri 的 opener 与 `open::that` 只会把路径交给默认应用，给不了这些控制。

## 决策

**macOS 上开终端与开目录都写成 AppleScript，必要时配 `open -na`；脚本失败如实报错。**

1. 终端按设置分两种，集中在 `open_terminal_with`：
   - Ghostty：`open -na Ghostty --args --title=<标签> --command=<命令>` 开一个新实例并在
     里面执行命令，再用 `osascript -e 'tell application "Ghostty" to activate'` 带到前台；
     `open` 能不能起来就是结果，它的退出码不判。
   - Terminal.app：一段 AppleScript。Terminal 冷启动时会自己开一个默认窗口，所以先
     `activate`、循环等默认窗口出现（20 次 × 0.1 秒），再在它的第一个选项卡里 `do script`；
     已经开着窗口就直接 `do script`（新窗口）再 activate。冷启动那条不做这件事就会得到两个
     窗口：一个空的默认窗口，和我们的那个。
2. 访达打开目录（`open_folder_in_file_manager`）：先把路径 `canonicalize`（`/tmp` →
   `/private/tmp`），再按末尾带 `/` 的 POSIX 路径与每个访达窗口的 target 比；命中就把那个
   窗口提到最前、取消折叠、activate，不新开窗口。
3. 没有命中时：一个窗口都没有就新建窗口；有窗口就合成 ⌘T 开新选项卡
   （`tell application "System Events" to keystroke "t" using command down`，再设置新选项卡
   的 target）。
4. 合成按键要辅助功能（Accessibility）权限：没有权限时 System Events 会报错，脚本捕获这个
   错误，退回「新建访达窗口」，不把这次打开算失败。
5. 在访达里定位日志文件仍用 `open -R`（要让文件被选中，不需要选项卡逻辑）；文件还不存在
   时退回打开日志目录。

## 后果

- 复用访达选项卡依赖用户授予辅助功能权限；没给权限时行为降一档（新窗口），不是错误——两者
  对用户都是「目录打开了」，调用方不区分。
- 这两个平台细节固定在 `open_terminal_with` 与 `open_folder_in_file_manager`，其他平台的
  分支是 `open::that`；托盘菜单里的打开目录仍走 `open::that`，不经过这里。
- 比较必须用规范化后的路径：不先 canonicalize，`/tmp` 与 `/private/tmp` 会算成两个目录，
  复用永远不会命中。
- 终端命令行要过两层引号：Ghostty 的 `--command` 与 AppleScript 字符串各一层，多一个双引号
  就会提前结束脚本（引号规则见 ADR-0018）。
- 失败不静默：osascript 的 stderr 原样回到页面（「打开终端失败: …」）。

## 备选方案

- **用 opener / `open` 打开**：被否。终端里跑什么命令、访达开在哪、要不要复用选项卡，这三件
  事它都不管。
- **改用 AppKit 接口直接调 NSWorkspace / NSAppleScript**：被否。做的事一样，还要多写一层
  原生桥接，AppleScript 的错误信息反而更接近用户能看懂的话。
- **访达一律开新窗口**：被否。同一个目录反复打开会攒一屏窗口，而复用选项卡正是这个动作的意义。

## 追加（2026-11 注释清理）

- 远端实例的终端与日志动作都先问那台机器一次：主机不可达、或那边的 DSH_HOME 已经不在时，页面得到一句带机器名的原因，而不是打开一个随即退出的终端窗口；本机会话写 scripts/term_<实例 id>.sh 交给终端，远端会话不落任何本机文件。日志不在本机：远端实例的日志在那台机器上，而 open -R 只认本地文件，所以先把尾部（remote::LOG_TAIL_BYTES）取回启动器自己的日志目录、每次打开覆盖成 <实例 id>.remote.log 再揭示——它是尾巴的拷贝，不是第二份要跟随的日志。
