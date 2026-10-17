mod app_menu;
mod applog;
mod commands;
mod config;
mod doctor;
mod icons;
mod launch;
mod native;
mod plugins;
mod process;
mod profile;
mod proxy;
mod remote;
mod runtime;
mod ssh;
mod tasks;
mod toolchain;
mod tray;
mod tunnel;
mod update;
mod windows;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tauri::{Emitter, Manager};

pub struct AppState {
    pub config_path: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub config: StdMutex<config::Config>,
    pub running: tokio::sync::Mutex<HashMap<String, process::RunningInstance>>,
    pub tasks: tokio::sync::Mutex<HashMap<String, tasks::TaskInfo>>,
    /// The one boundary every remote action crosses. A test substitutes an
    /// in-memory double here instead of needing a real machine.
    pub host_transport: Arc<dyn ssh::HostTransport>,
    /// The port forwards to remote hosts this launcher session opened, so the
    /// exit path can withdraw exactly what it opened and nothing else
    /// (ADR-0008). Launcher state, not machine state: they live and die with
    /// this process.
    pub forwards: tunnel::Forwards,
    /// What the launcher last learned about reaching each host. Runtime only:
    /// 「现在连不上」 is a fact about the network and the moment, not part of the
    /// host record — a machine that is switched off is still a machine the user
    /// saved (ADR-0006).
    pub host_statuses: StdMutex<HashMap<String, config::HostStatus>>,
    /// Remote hosts whose instances this session has not checked yet. Until a
    /// host leaves this set, nothing is known about its instances, and the page
    /// reads that as 「启动中」 — never as 「已停止」, which is the state that makes
    /// a user start a second DSH on the server (ADR-0007 后果).
    pub remote_scan: StdMutex<std::collections::HashSet<String>>,
}

impl AppState {
    /// The status last observed for one host. `None` before the first test: the
    /// page then shows no badge at all rather than pretending 「不可达」.
    pub fn host_status(&self, host_id: &str) -> Option<config::HostStatus> {
        self.host_statuses.lock().unwrap().get(host_id).cloned()
    }

    pub fn set_host_status(&self, host_id: &str, status: config::HostStatus) {
        self.host_statuses
            .lock()
            .unwrap()
            .insert(host_id.to_string(), status);
    }

    /// Why the launcher currently cannot say anything about one host's instances
    /// — `None` when the last word was 「到得了」, or when there is no word at all.
    ///
    /// The one definition of 「主机不可达」, and the one predicate on it: the
    /// instance list, the plugin page, the bulk buttons and the tray all ask
    /// this question, and two answers would let two of them disagree about the
    /// same machine. 本机 is never in this state — nothing ever sets one for it.
    pub fn host_unreachable_reason(&self, host_id: &str) -> Option<String> {
        match self.host_status(host_id) {
            Some(status) if status.state == config::HostState::Unreachable => Some(
                status
                    .message
                    .map(|message| message.trim().to_string())
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "那台机器没有回应".to_string()),
            ),
            _ => None,
        }
    }

    /// A deleted host takes its status with it: statuses are keyed by host id,
    /// and a host added later is not the same machine.
    pub fn forget_host_status(&self, host_id: &str) {
        self.host_statuses.lock().unwrap().remove(host_id);
    }
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
                host_transport: Arc::new(ssh::SshTransport::new()),
                host_statuses: StdMutex::new(HashMap::new()),
                remote_scan: StdMutex::new(std::collections::HashSet::new()),
                forwards: tunnel::Forwards::default(),
            });

            // Resolve the toolchain once at startup and persist it, so the
            // first spawn of any instance uses a known absolute node/pnpm
            // rather than whatever PATH this process happens to have. A stale
            // binding (a deleted nvm version) is re-probed inside.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<AppState>();
                    let binding = runtime::refresh_toolchain_binding(&state, false).await;
                    crate::log_info!(
                        "工具链绑定：node={}，pnpm={}",
                        binding
                            .node
                            .as_ref()
                            .map(|b| format!("{}（{}）", b.version, b.path.display()))
                            .unwrap_or_else(|| "未找到".to_string()),
                        binding
                            .pnpm
                            .as_ref()
                            .map(|b| format!("{}（{}）", b.version, b.path.display()))
                            .unwrap_or_else(|| "未找到".to_string()),
                    );
                });
            }

            // System tray with dynamic menu.
            tray::build_tray(app.handle())?;

            // Instances that outlived the last launcher session (an update, a
            // force quit) are adopted in the background, so the board, the tray
            // and the plugins page all agree they are running. 本机's instances
            // are found by probing this Mac's ports; every remote host's by
            // asking that machine (ADR-0007 §2) — which is also why the remote
            // hosts are marked 「还没查」 first: until a machine has answered,
            // its instances are unknown, not stopped.
            {
                let handle = app.handle().clone();
                let state = handle.state::<AppState>();
                crate::process::begin_remote_scan(&state);
                tauri::async_runtime::spawn(async move {
                    crate::process::adopt_surviving_instances(&handle).await;
                });
            }
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    crate::process::adopt_surviving_remote_instances(&handle).await;
                });
            }

            // Close-to-tray for the main window (destroyed-window recreation
            // is handled by show_or_create_main / the RunEvent loop below).
            if let Some(win) = app.get_webview_window("main") {
                windows::attach_close_behavior(app.handle(), &win);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::create_host,
            commands::update_host,
            commands::delete_host,
            commands::test_host_connection,
            commands::list_ssh_aliases,
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
            runtime::set_toolchain_override,
            commands::list_instances,
            commands::create_instance,
            commands::delete_instance,
            commands::update_instance,
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
            commands::refresh_instance_status,
            commands::open_instance_window,
            commands::open_external,
            commands::open_launcher_directory,
            commands::open_launcher_log,
            commands::open_instance_log,
            commands::open_instance_directory,
            commands::instance_remote_location,
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
            plugins::list_plugins,
            plugins::set_bundle_enabled,
            plugins::remove_bundle,
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
            // instance is left orphaned, and withdraw the port forwards this
            // session opened — the ssh connection is the launcher's own, so
            // nothing the user configured is touched (ADR-0008 §1).
            tauri::RunEvent::Exit => {
                let state = app_handle.state::<AppState>();
                process::kill_all(&state);
                tunnel::close_all_blocking(&state);
            }
            _ => {}
        });
}
