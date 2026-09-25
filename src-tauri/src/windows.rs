use tauri::{AppHandle, Manager, WebviewUrl, WindowEvent};

use crate::AppState;

/// Attaches the main window's close behavior: with `minimize_to_tray` the
/// close button hides the window instead of closing it; otherwise the window
/// is really destroyed, but the app itself stays alive (see the
/// `ExitRequested` handler in lib.rs) so instances keep running and the
/// window can be recreated from the Dock or the tray.
pub fn attach_close_behavior(app: &AppHandle, win: &tauri::WebviewWindow) {
    let handle = app.clone();
    let win2 = win.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            let minimize = handle
                .state::<AppState>()
                .config
                .lock()
                .unwrap()
                .settings
                .minimize_to_tray;
            if minimize {
                api.prevent_close();
                let _ = win2.hide();
            }
        }
    });
}

/// Recreates the main launcher window (with its close handler) after it was
/// destroyed by closing with `minimize_to_tray` off.
fn create_main_window(app: &AppHandle) -> Result<(), String> {
    let win = tauri::WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("DSH Launcher")
        .inner_size(1080.0, 760.0)
        .min_inner_size(900.0, 620.0)
        .center()
        .visible(true)
        .decorations(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(20.0, 28.0))
        .build()
        .map_err(|e| e.to_string())?;
    attach_close_behavior(app, &win);
    Ok(())
}

/// Shows the main launcher window, recreating it when it no longer exists.
/// Used by the Dock `Reopen` event and the tray's "打开启动器" entry.
pub fn show_or_create_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    } else if let Err(e) = create_main_window(app) {
        crate::log_warn!("重建主窗口失败: {e}");
    }
}
