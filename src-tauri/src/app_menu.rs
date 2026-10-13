//! macOS 原生应用菜单栏 + 应用快捷键后端 (t3)。
//!
//! 在 macOS 上构建标准应用菜单 (App / 编辑 / 显示 / 窗口 / 帮助)，
//! 含 About / 偏好设置 Cmd+, / 隐藏 Cmd+H / 退出 Cmd+Q /
//! 最小化 Cmd+M / 关闭窗口 Cmd+W 等标准加速键。
//! 其中 PredefinedMenuItem 提供原生行为与加速键，
//! 自定义 MenuItem 用于导航与功能项并转发事件到前端。
//!
//! ## 前端契约 (M3 收敛后的唯一规范)
//!
//! 导航统一走 `menu-navigate` 事件（载荷为路由字符串，前端用
//! `@tauri-apps/api/event` 的 `listen` 订阅后交 `resolveMenuRoute` 解析）：
//!
//! | `menu-navigate` 载荷 | 菜单来源 |
//! | --- | --- |
//! | `/` | “显示”→实例 (Cmd+1) |
//! | `/library?tab=versions` | “显示”→DSH 版本 (Cmd+2) |
//! | `/library?tab=homes` | “显示”→DSH_HOME (Cmd+3) |
//! | `/library?tab=profiles` | “显示”→Profile (Cmd+4) |
//! | `/settings` | App 菜单→偏好设置 (Cmd+,) |
//!
//! 插件管理不占顶级导航：它属于运行中的实例，入口只在实例侧两处（详情页头部
//! 按钮与实例行展开卡片里的按钮），跳转带 `?instanceId=` 以便返回。`/tasks` 保留
//! 为可用的历史链接，前端把它重定向到 `/versions`（安装进度就显示在那个版本那
//! 一行上）。
//!
//! 刷新没有菜单项：实例状态由后端事件推送，`⌘R` 走前端 `handleAppKeydown`，
//! 用于「在启动器外面改了什么」的少数场景。
//!
//! 非导航动作仍用各自事件（载荷为 `()`）：
//!
//! | 事件 | 菜单来源 | 建议前端动作 |
//! | --- | --- | --- |
//! | `check-update` | “帮助”→检查更新 | 调用 `check_launcher_update` |
//! | `open-help` | “帮助”→使用文档 | 打开文档页面 |
//!
//! “显示主窗口”(Cmd+0)、导航类菜单会先在后端
//! 调用 `windows::show_or_create_main` 确保窗口可见再 emit。
//!
//! ## 关于 Quit 的说明
//!
//! “退出”菜单项故意不用 `PredefinedMenuItem::quit`: 原生 quit 产生的 ExitRequested
//! (code 为 None) 会被 lib.rs 的保活处理器 (`api.prevent_exit()`) 吃掉——这是
//! 托盘常驻设计的一部分。自定义项保留标准 Cmd+Q 加速键但走托盘一致的
//! 完全退出路径 (`process::kill_all` + `app.exit(0)`)。

use tauri::menu::{
    AboutMetadataBuilder, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Emitter, Manager};

// ── 自定义菜单项 id ─────────────────────────────────────────────
const ID_PREFERENCES: &str = "appmenu-preferences";
const ID_QUIT: &str = "appmenu-quit";
const ID_VIEW_HOME: &str = "appmenu-view-home";
const ID_VIEW_HOMES: &str = "appmenu-view-homes";
const ID_VIEW_PROFILES: &str = "appmenu-view-profiles";
const ID_VIEW_VERSIONS: &str = "appmenu-view-versions";
const ID_VIEW_SETTINGS: &str = "appmenu-view-settings";
const ID_SHOW_MAIN: &str = "appmenu-show-main";
const ID_CHECK_UPDATE: &str = "appmenu-check-update";
const ID_OPEN_HELP: &str = "appmenu-open-help";

// ── 转发到前端的事件名（M3 收敛：导航只用 `menu-navigate`） ──
/// 规范导航事件（`shortcuts.ts` 的 `MENU_NAVIGATE_EVENTS` 之首），
/// 载荷为路由字符串（`/`、`/homes`、`/profiles`、`/versions`、`/settings`）。
pub const EVT_MENU_NAVIGATE: &str = "menu-navigate";
/// 检查更新——前端建议调用 `check_launcher_update`。
pub const EVT_CHECK_UPDATE: &str = "check-update";
/// 使用文档——前端建议打开文档页面。
pub const EVT_OPEN_HELP: &str = "open-help";

/// 构建 macOS 原生应用菜单。在 Windows/Linux 上同样显示
/// 一致的菜单栏 (不受支持的 predefined 项在那些平台上为 no-op)，
/// macOS 上第一个菜单会自动使用应用名。
/// Window / Help 子菜单使用 Tauri 标准 id
/// (`WINDOW_SUBMENU_ID` / `HELP_SUBMENU_ID`) 以获得原生集成。
pub fn build_app_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    // ── App 菜单 (About / 偏好设置 / 服务 / 隐藏 / 退出) ─────────────────
    let about_meta = AboutMetadataBuilder::new()
        .name(Some("DSH Launcher"))
        .version(Some(env!("CARGO_PKG_VERSION").to_string()))
        .copyright(Some("DSH Launcher"))
        .build();
    let about = PredefinedMenuItem::about(app, Some("关于 DSH Launcher"), Some(about_meta))?;
    let preferences =
        MenuItem::with_id(app, ID_PREFERENCES, "偏好设置…", true, Some("CmdOrCtrl+,"))?;
    let services = PredefinedMenuItem::services(app, Some("服务"))?;
    let hide = PredefinedMenuItem::hide(app, Some("隐藏 DSH Launcher"))?;
    let hide_others = PredefinedMenuItem::hide_others(app, Some("隐藏其他"))?;
    let show_all = PredefinedMenuItem::show_all(app, Some("显示全部"))?;
    // 注意见模块文档——不用 predefined quit 以避免被保活处理器吃掉。
    let quit = MenuItem::with_id(app, ID_QUIT, "退出 DSH Launcher", true, Some("CmdOrCtrl+Q"))?;
    let sep_a1 = PredefinedMenuItem::separator(app)?;
    let sep_a2 = PredefinedMenuItem::separator(app)?;
    let sep_a3 = PredefinedMenuItem::separator(app)?;
    let sep_a4 = PredefinedMenuItem::separator(app)?;
    let app_submenu = Submenu::with_items(
        app,
        "DSH Launcher",
        true,
        &[
            &about,
            &sep_a1,
            &preferences,
            &sep_a2,
            &services,
            &sep_a3,
            &hide,
            &hide_others,
            &show_all,
            &sep_a4,
            &quit,
        ],
    )?;

    // ── 编辑 (全 predefined 原生行为) ─────────────────────────────
    let undo = PredefinedMenuItem::undo(app, Some("撤销"))?;
    let redo = PredefinedMenuItem::redo(app, Some("重做"))?;
    let cut = PredefinedMenuItem::cut(app, Some("剪切"))?;
    let copy = PredefinedMenuItem::copy(app, Some("复制"))?;
    let paste = PredefinedMenuItem::paste(app, Some("粘贴"))?;
    let select_all = PredefinedMenuItem::select_all(app, Some("全选"))?;
    let sep_e = PredefinedMenuItem::separator(app)?;
    let edit_submenu = Submenu::with_items(
        app,
        "编辑",
        true,
        &[&undo, &redo, &sep_e, &cut, &copy, &paste, &select_all],
    )?;

    // ── 显示 (应用内导航快捷键→转发前端) ──────────────────
    // 顶层只有三个目的地（实例 / 资源库 / 设置）；资源库的三个标签页各占一格，
    // 所以 DSH 版本 / DSH_HOME / Profile 仍然一键直达。插件管理不占位：它属于
    // 运行中的实例，入口在实例页那一行展开的卡片里。
    let view_home = MenuItem::with_id(app, ID_VIEW_HOME, "实例", true, Some("CmdOrCtrl+1"))?;
    let view_versions = MenuItem::with_id(
        app,
        ID_VIEW_VERSIONS,
        "DSH 版本",
        true,
        Some("CmdOrCtrl+2"),
    )?;
    let view_homes = MenuItem::with_id(app, ID_VIEW_HOMES, "DSH_HOME", true, Some("CmdOrCtrl+3"))?;
    let view_profiles = MenuItem::with_id(
        app,
        ID_VIEW_PROFILES,
        "Profile",
        true,
        Some("CmdOrCtrl+4"),
    )?;
    // “设置”不重复占用 Cmd+,，加速键归偏好设置独占。
    let view_settings = MenuItem::with_id(app, ID_VIEW_SETTINGS, "设置", true, None::<&str>)?;
    let sep_v = PredefinedMenuItem::separator(app)?;
    let view_submenu = Submenu::with_items(
        app,
        "显示",
        true,
        &[
            &view_home,
            &view_homes,
            &view_profiles,
            &view_versions,
            &sep_v,
            &view_settings,
        ],
    )?;

    // ── 窗口 (标准 id 获得原生集成) ────────────────────────
    let show_main = MenuItem::with_id(app, ID_SHOW_MAIN, "显示主窗口", true, Some("CmdOrCtrl+0"))?;
    let minimize = PredefinedMenuItem::minimize(app, Some("最小化"))?;
    let maximize = PredefinedMenuItem::maximize(app, Some("缩放"))?;
    let fullscreen = PredefinedMenuItem::fullscreen(app, Some("进入全屏幕"))?;
    let front = PredefinedMenuItem::bring_all_to_front(app, Some("前置全部窗口"))?;
    let close_window = PredefinedMenuItem::close_window(app, Some("关闭窗口"))?;
    let sep_w1 = PredefinedMenuItem::separator(app)?;
    let sep_w2 = PredefinedMenuItem::separator(app)?;
    let sep_w3 = PredefinedMenuItem::separator(app)?;
    let window_submenu = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "窗口",
        true,
        &[
            &show_main,
            &sep_w1,
            &minimize,
            &maximize,
            &fullscreen,
            &sep_w2,
            &front,
            &sep_w3,
            &close_window,
        ],
    )?;

    // ── 帮助 (标准 id) ───────────────────────────────────────
    let check_update = MenuItem::with_id(app, ID_CHECK_UPDATE, "检查更新", true, None::<&str>)?;
    let open_help = MenuItem::with_id(app, ID_OPEN_HELP, "使用文档", true, None::<&str>)?;
    let help_submenu = Submenu::with_id_and_items(
        app,
        HELP_SUBMENU_ID,
        "帮助",
        true,
        &[&check_update, &open_help],
    )?;

    let items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &app_submenu,
        &edit_submenu,
        &view_submenu,
        &window_submenu,
        &help_submenu,
    ];
    Menu::with_items(app, &items)
}

/// 原生菜单事件分发——窗口操作直接在后端处理，
/// 导航/功能类 emit 事件给前端。不认识的 id 忽略
/// (predefined 项如 About/隐藏/剪切等由系统原生处理)。
pub fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    // 先确保主窗口可见，再用唯一的规范事件 `menu-navigate` 通知前端路由
    // 跳转（路由字符串由前端 `resolveMenuRoute` 解析）。M3 收敛：不再发送
    // t3 时期的专有导航事件，托盘侧亦然。
    let show_and_emit = |app: &AppHandle, route: &str| {
        crate::windows::show_or_create_main(app);
        let _ = app.emit(EVT_MENU_NAVIGATE, route.to_string());
    };
    match event.id().as_ref() {
        ID_SHOW_MAIN => crate::windows::show_or_create_main(app),
        ID_VIEW_HOME => show_and_emit(app, "/"),
        ID_VIEW_VERSIONS => show_and_emit(app, "/library?tab=versions"),
        ID_VIEW_HOMES => show_and_emit(app, "/library?tab=homes"),
        ID_VIEW_PROFILES => show_and_emit(app, "/library?tab=profiles"),
        ID_VIEW_SETTINGS | ID_PREFERENCES => show_and_emit(app, "/settings"),
        ID_CHECK_UPDATE => {
            if let Err(e) = app.emit(EVT_CHECK_UPDATE, ()) {
                crate::log_warn!("菜单事件 `check-update` 转发失败: {e}");
            }
        }
        ID_OPEN_HELP => {
            if let Err(e) = app.emit(EVT_OPEN_HELP, ()) {
                crate::log_warn!("菜单事件 `open-help` 转发失败: {e}");
            }
        }
        ID_QUIT => {
            let state = app.state::<crate::AppState>();
            crate::process::kill_all(&state);
            app.exit(0);
        }
        _ => {}
    }
}
