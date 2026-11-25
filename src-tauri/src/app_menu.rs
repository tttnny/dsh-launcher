use tauri::menu::{
    AboutMetadataBuilder, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Emitter, Manager};

const ID_PREFERENCES: &str = "appmenu-preferences";
const ID_QUIT: &str = "appmenu-quit";
const ID_VIEW_HOME: &str = "appmenu-view-home";
const ID_VIEW_INSTALL: &str = "appmenu-view-install";
const ID_VIEW_DATA: &str = "appmenu-view-data";
const ID_VIEW_MACHINES: &str = "appmenu-view-machines";
const ID_VIEW_SETTINGS: &str = "appmenu-view-settings";
const ID_SHOW_MAIN: &str = "appmenu-show-main";
const ID_CHECK_UPDATE: &str = "appmenu-check-update";
const ID_OPEN_HELP: &str = "appmenu-open-help";

pub const EVT_MENU_NAVIGATE: &str = "menu-navigate";
pub const EVT_CHECK_UPDATE: &str = "check-update";
pub const EVT_OPEN_HELP: &str = "open-help";

/// 构建 macOS 原生应用菜单。在 Windows/Linux 上同样显示
/// 一致的菜单栏 (不受支持的 predefined 项在那些平台上为 no-op)，
/// macOS 上第一个菜单会自动使用应用名。
/// Window / Help 子菜单使用 Tauri 标准 id
/// (`WINDOW_SUBMENU_ID` / `HELP_SUBMENU_ID`) 以获得原生集成。
pub fn build_app_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
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

    let view_home = MenuItem::with_id(app, ID_VIEW_HOME, "实例", true, Some("CmdOrCtrl+1"))?;
    let view_install = MenuItem::with_id(app, ID_VIEW_INSTALL, "安装", true, Some("CmdOrCtrl+2"))?;
    let view_data = MenuItem::with_id(app, ID_VIEW_DATA, "数据", true, Some("CmdOrCtrl+3"))?;
    let view_machines =
        MenuItem::with_id(app, ID_VIEW_MACHINES, "机器", true, Some("CmdOrCtrl+4"))?;
    let view_settings = MenuItem::with_id(app, ID_VIEW_SETTINGS, "设置", true, None::<&str>)?;
    let sep_v = PredefinedMenuItem::separator(app)?;
    let view_submenu = Submenu::with_items(
        app,
        "显示",
        true,
        &[
            &view_home,
            &view_install,
            &view_data,
            &view_machines,
            &sep_v,
            &view_settings,
        ],
    )?;

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

/// Newly created windows check on startup. An existing window can refresh
/// immediately and show its result in the title bar.
pub(crate) fn request_update_check(app: &AppHandle) {
    let had_window = app.get_webview_window("main").is_some();
    crate::windows::show_or_create_main(app);
    if had_window {
        if let Err(e) = app.emit(EVT_CHECK_UPDATE, ()) {
            crate::log_warn!("菜单事件 `check-update` 转发失败: {e}");
        }
    }
}

pub fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let show_and_emit = |app: &AppHandle, route: &str| {
        crate::windows::show_or_create_main(app);
        let _ = app.emit(EVT_MENU_NAVIGATE, route.to_string());
    };
    match event.id().as_ref() {
        ID_SHOW_MAIN => crate::windows::show_or_create_main(app),
        ID_VIEW_HOME => show_and_emit(app, "/"),
        ID_VIEW_INSTALL => show_and_emit(app, "/install"),
        ID_VIEW_DATA => show_and_emit(app, "/data"),
        ID_VIEW_MACHINES => show_and_emit(app, "/machines"),
        ID_VIEW_SETTINGS | ID_PREFERENCES => show_and_emit(app, "/settings"),
        ID_CHECK_UPDATE => request_update_check(app),
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
