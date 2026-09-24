mod app_menu;
mod applog;
mod commands;
mod config;
mod doctor;
mod icons;
mod launch;
mod plugins;
mod process;
mod profile;
mod proxy;
mod runtime;
mod tasks;
mod toolchain;
mod traffic;
mod tray;
mod update;
mod windows;

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use tauri::{Emitter, Manager};

pub struct AppState {
    pub config_path: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub config: StdMutex<config::Config>,
    pub running: tokio::sync::Mutex<HashMap<String, process::RunningInstance>>,
    pub tasks: tokio::sync::Mutex<HashMap<String, tasks::TaskInfo>>,
}

/// Extracts a `dsh-launcher://…` deep link from process arguments.
pub(crate) fn deep_link_from_args(args: &[String]) -> Option<String> {
    args.iter()
        .find(|a| a.starts_with("dsh-launcher://"))
        .cloned()
}

/// Pending cold-start deep link: the frontend pulls this once the webview is
/// ready (events emitted before that would be lost).
#[tauri::command]
fn pending_deep_link() -> Option<String> {
    deep_link_from_args(&std::env::args().collect::<Vec<_>>())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Single instance first: a second launch (e.g. browser protocol
        // activation) forwards its argv to the running instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let link = deep_link_from_args(&argv);
            // launch links are headless: start the instance without popping
            // the launcher window up (issue #9).
            let is_launch = link
                .as_deref()
                .map(|u| u.starts_with("dsh-launcher://launch"))
                .unwrap_or(false);
            if !is_launch {
                windows::show_or_create_main(app);
            }
            if let Some(url) = link {
                crate::log_info!("单实例转发 deep link: {url}");
                let _ = app.emit("deep-link", url);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // macOS 原生应用菜单栏 (App/编辑/显示/窗口/帮助) + 应用快捷键 (t3)。
        .menu(crate::app_menu::build_app_menu)
        .on_menu_event(crate::app_menu::handle_menu_event)
        .setup(|app| {
            // Register the dsh-launcher:// scheme at runtime (Windows/Linux)
            // and forward every deep link to the frontend; the modpack
            // import flow consumes dsh-launcher://pack?url=<tgz>.
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register("dsh-launcher") {
                    crate::log_warn!("注册 dsh-launcher:// 协议失败: {e}");
                }
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        crate::log_info!("收到 deep link: {url}");
                        let _ = handle.emit("deep-link", url.to_string());
                    }
                });
            }
            // Cold start from a launch shortcut stays silent: hide the main
            // window and let the frontend start the instance (issue #9).
            if deep_link_from_args(&std::env::args().collect::<Vec<_>>())
                .map(|u| u.starts_with("dsh-launcher://launch"))
                .unwrap_or(false)
            {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.hide();
                }
            }
            // Populate common macOS environment paths so GUI launcher can locate tools
            runtime::ensure_macos_paths();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            // A managed Node.js installed by a previous one-click install
            // (issue #23) joins PATH for everything the launcher spawns.
            runtime::ensure_local_node_on_path(&data_dir);
            let config_path = data_dir.join("config.json");
            let cfg = config::load_config(&config_path);
            proxy::sync_from_settings(&cfg.settings);

            // Runtime log: rotate the previous latest.log, then apply the
            // configured level (invalid stored values fall back to info).
            let log_level =
                applog::parse_level(&cfg.settings.log_level).unwrap_or(applog::Level::Info);
            if let Err(e) = applog::init(&data_dir.join("logs"), log_level) {
                eprintln!("dsh-launcher: 初始化运行日志失败: {e}");
            }
            crate::log_info!(
                "启动器已启动，版本 {}，数据目录 {}",
                env!("CARGO_PKG_VERSION"),
                data_dir.display()
            );

            app.manage(AppState {
                config_path,
                data_dir: data_dir.clone(),
                config: StdMutex::new(cfg),
                running: tokio::sync::Mutex::new(HashMap::new()),
                tasks: tokio::sync::Mutex::new(HashMap::new()),
            });

            // System tray with dynamic menu.
            tray::build_tray(app.handle())?;

            // Close-to-tray for the main window (destroyed-window recreation
            // is handled by show_or_create_main / the RunEvent loop below).
            if let Some(win) = app.get_webview_window("main") {
                windows::attach_close_behavior(app.handle(), &win);
                // Hide the native traffic lights; the sidebar draws its own,
                // aligned with the sidebar controls.
                traffic::attach(&win);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_homes,
            commands::create_home,
            commands::default_dedicated_home_path,
            commands::remove_home,
            commands::list_versions,
            commands::fetch_available_versions,
            commands::remove_version,
            tasks::start_install_version_task,
            tasks::list_tasks,
            tasks::remove_task,
            tasks::cancel_task,
            runtime::get_runtime_status,
            runtime::start_install_node_task,
            commands::list_instances,
            commands::update_instance,
            commands::set_instance_port,
            commands::list_profiles,
            commands::create_profile,
            commands::copy_profile,
            commands::rename_profile,
            commands::delete_profile,
            commands::start_instance,
            commands::stop_instance,
            commands::restart_instance,
            commands::check_instance_health,
            commands::list_instance_status,
            commands::open_instance_window,
            commands::open_external,
            commands::open_launcher_directory,
            commands::open_launcher_log,
            commands::open_instance_log,
            commands::open_instance_directory,
            commands::open_home_directory,
            commands::get_launcher_directory,
            pending_deep_link,
            icons::set_instance_icon,
            icons::clear_instance_icon,
            icons::read_instance_icon,
            commands::get_settings,
            commands::update_settings,
            commands::open_instance_terminal,
            update::check_launcher_update,
            plugins::list_installed_plugins,
            plugins::set_plugins_enabled,
            plugins::uninstall_plugin,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            // macOS: clicking the Dock icon must bring the main window back —
            // both when it was hidden (close-to-tray) and when it was really
            // destroyed (minimize_to_tray off); the latter rebuilds it.
            tauri::RunEvent::Reopen { .. } => {
                windows::show_or_create_main(app_handle);
            }
            // The main window (or the last instance window) was closed with
            // `minimize_to_tray` off: keep the app alive so running DSH
            // instances are not torn down and the window can be recreated
            // from the Dock/tray. Explicit exits (tray quit, app.exit())
            // carry `code: Some(_)` and are not prevented.
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => {
                api.prevent_exit();
            }
            // Terminate child processes when the launcher exits so no DSH
            // instance is left orphaned.
            tauri::RunEvent::Exit => {
                let state = app_handle.state::<AppState>();
                process::kill_all(&state);
            }
            _ => {}
        });
}
