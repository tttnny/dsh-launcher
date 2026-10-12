use crate::{process, AppState};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const MENU_OPEN_LAUNCHER: &str = "open-launcher";
const MENU_QUIT: &str = "quit";
const MENU_RUNNING_SUB: &str = "running-profiles";
const MENU_OPEN_PREFIX: &str = "open::";
const MENU_STOP_PREFIX: &str = "stop::";
// t5: enriched tray menu ids. Original ids above are kept verbatim for
// merge compatibility with the tray-click branch (t2).
const MENU_STATUS: &str = "status-header";
const MENU_EMPTY: &str = "empty-hint";
const MENU_QUICK_SUB: &str = "quick-entries";
const MENU_QUICK_HOME: &str = "quick::home";
const MENU_QUICK_HOMES: &str = "quick::homes";
const MENU_QUICK_PROFILES: &str = "quick::profiles";
const MENU_QUICK_VERSIONS: &str = "quick::versions";
const MENU_START_ALL: &str = "start-all";
const MENU_STOP_ALL: &str = "stop-all";
const MENU_OPEN_DATA_DIR: &str = "open-data-dir";
const MENU_OPEN_LOG: &str = "open-log";
const MENU_CHECK_UPDATE: &str = "check-update";
const MENU_OPEN_SETTINGS: &str = "open-settings";
const MENU_RESTART: &str = "restart";

/// (instance_id, instance_name, profile)
type RunningItem = (String, String, String);

/// M1: 左键单击防抖 —— 双击会先产生两次 Click(Left,Up) 再产生 DoubleClick，
/// 若单击立即开窗，双击就会多弹出一次主窗口。单击只记录时间戳并延迟
/// ~250ms，若期间出现新的单击或双击则丢弃本次单击。
const LEFT_CLICK_DEBOUNCE_MS: u64 = 250;
static LAST_LEFT_UP_MS: AtomicU64 = AtomicU64::new(0);
static LAST_DOUBLE_CLICK_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Builds the tray icon with its dynamic menu. Called from setup with no
/// running instances yet.
pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &[])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().unwrap())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == MENU_OPEN_LAUNCHER {
                show_launcher(app);
            } else if id == MENU_QUICK_HOME {
                show_and_navigate(app, "/");
            } else if id == MENU_QUICK_HOMES {
                show_and_navigate(app, "/homes");
            } else if id == MENU_QUICK_PROFILES {
                show_and_navigate(app, "/profiles");
            } else if id == MENU_QUICK_VERSIONS {
                show_and_navigate(app, "/versions");
            } else if id == MENU_START_ALL {
                start_all_instances(app);
            } else if id == MENU_STOP_ALL {
                stop_all_instances(app);
            } else if id == MENU_OPEN_DATA_DIR {
                open_data_dir(app);
            } else if id == MENU_OPEN_LOG {
                open_launcher_log(app);
            } else if id == MENU_CHECK_UPDATE {
                check_update_from_tray(app);
            } else if id == MENU_OPEN_SETTINGS {
                show_and_navigate(app, "/settings");
            } else if id == MENU_RESTART {
                app.restart();
            } else if id == MENU_QUIT {
                quit(app);
            } else if let Some(instance_id) = id.strip_prefix(MENU_OPEN_PREFIX) {
                // NOTE: "quick::" ids contain "::" too but never start with
                // "open::", so this branch cannot swallow quick entries.
                open_instance_from_tray(app, instance_id);
            } else if let Some(instance_id) = id.strip_prefix(MENU_STOP_PREFIX) {
                stop_instance_from_tray(app, instance_id);
            }
        })
        .on_tray_icon_event(|tray, event| {
            // t2: 左键单击打开主窗口；右键单击交由系统弹出托盘菜单
            // (show_menu_on_left_click(false) 下左键默认无行为，右键自动弹菜单)；
            // 双击保留原有兼容行为（最近实例 / 单实例 / 启动器）。
            // M1: 单击经防抖延迟执行，双击优先 —— 双击序列会先触发两次
            // Click(Left,Up)，若单击立即开窗，双击就会多弹一次主窗口。
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    let stamp = now_ms();
                    LAST_LEFT_UP_MS.store(stamp, Ordering::SeqCst);
                    let handle = tray.app_handle().clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            LEFT_CLICK_DEBOUNCE_MS,
                        ))
                        .await;
                        // 期间有新的单击则让新的一次决定；有双击则双击接管。
                        if LAST_LEFT_UP_MS.load(Ordering::SeqCst) != stamp {
                            return;
                        }
                        if LAST_DOUBLE_CLICK_MS.load(Ordering::SeqCst) > stamp {
                            return;
                        }
                        show_launcher(&handle);
                    });
                }
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    // No-op: the menu attached via TrayIconBuilder pops up
                    // automatically on right click. Kept explicit so a later
                    // reader does not "fix" right click into opening windows.
                }
                TrayIconEvent::DoubleClick { .. } => {
                    LAST_DOUBLE_CLICK_MS.store(now_ms(), Ordering::SeqCst);
                    handle_double_click(tray.app_handle());
                }
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}

/// Rebuilds the tray menu to reflect the current set of running instances.
/// Must be called from an async context.
pub async fn rebuild_tray_menu(app: &AppHandle) {
    let snapshot = running_snapshot(app).await;
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    match build_menu(app, &snapshot) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => {
            eprintln!("dsh-launcher: rebuild tray menu failed: {e}");
        }
    }
}

async fn running_snapshot(app: &AppHandle) -> Vec<RunningItem> {
    let state = app.state::<AppState>();
    let running = state.running.lock().await;
    let cfg = state.config.lock().unwrap();
    running
        .iter()
        .map(|(id, entry)| {
            let name = cfg
                .instances
                .iter()
                .find(|i| i.id == *id)
                .map(|i| i.name.clone())
                .unwrap_or_else(|| id.clone());
            (id.clone(), name, entry.profile.clone())
        })
        .collect()
}

fn build_menu(app: &AppHandle, running: &[RunningItem]) -> tauri::Result<Menu<tauri::Wry>> {
    // Total configured instances drive the empty copy and the
    // "start all" enabled state. Fall back to 0 before state exists.
    let total: usize = app
        .try_state::<AppState>()
        .map(|s| s.config.lock().unwrap().instances.len())
        .unwrap_or(0);

    // Status header (disabled): running count or empty copy.
    let status_text = if running.is_empty() {
        if total == 0 {
            "暂无实例，可在主窗口新建".to_string()
        } else {
            "暂无运行中的实例".to_string()
        }
    } else {
        format!("运行中 {} 个实例", running.len())
    };
    let status_header = MenuItem::with_id(app, MENU_STATUS, &status_text, false, None::<&str>)?;

    // M2: 与快捷入口>回到首页区分 —— 本项只显示主窗口（停留在当前页面），
    // 回到首页还会导航到启动页 "/"。菜单 id 保持不变，前端不受影响。
    let open_launcher = MenuItem::with_id(
        app,
        MENU_OPEN_LAUNCHER,
        "显示主窗口",
        true,
        Some("CmdOrCtrl+O"),
    )?;

    // Launcher page shortcuts: show the main window and ask the frontend to
    // navigate (event is a no-op until the frontend listens).
    let quick_home = MenuItem::with_id(app, MENU_QUICK_HOME, "回到首页", true, None::<&str>)?;
    let quick_homes = MenuItem::with_id(app, MENU_QUICK_HOMES, "HOME", true, None::<&str>)?;
    let quick_profiles = MenuItem::with_id(app, MENU_QUICK_PROFILES, "Profile", true, None::<&str>)?;
    let quick_versions = MenuItem::with_id(app, MENU_QUICK_VERSIONS, "版本", true, None::<&str>)?;
    // 插件与任务不再各占一格：插件管理在 Profile 行上，安装进度在版本那一行上。
    let quick_refs: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &quick_home,
        &quick_homes,
        &quick_profiles,
        &quick_versions,
    ];
    let quick_sub = Submenu::with_id_and_items(app, MENU_QUICK_SUB, "快捷入口", true, &quick_refs)?;

    // Running profiles live in a second-level submenu: each instance gets an
    // "open window" and a "stop" entry. Empty state is a disabled hint;
    // the status header above already says "暂无运行中".
    let mut owned: Vec<MenuItem<tauri::Wry>> = Vec::new();
    let mut running_sub = None;
    let mut empty_hint = None;
    if !running.is_empty() {
        for (id, name, profile) in running {
            owned.push(MenuItem::with_id(
                app,
                format!("{MENU_OPEN_PREFIX}{id}"),
                format!("打开：{name}（{profile}）"),
                true,
                None::<&str>,
            )?);
            owned.push(MenuItem::with_id(
                app,
                format!("{MENU_STOP_PREFIX}{id}"),
                format!("停止：{name}（{profile}）"),
                true,
                None::<&str>,
            )?);
        }
        let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = owned
            .iter()
            .map(|i| i as &dyn IsMenuItem<tauri::Wry>)
            .collect();
        running_sub = Some(Submenu::with_id_and_items(
            app,
            MENU_RUNNING_SUB,
            format!("运行中的 Profile ({})", running.len()),
            true,
            &refs,
        )?);
    } else {
        empty_hint = Some(MenuItem::with_id(
            app,
            MENU_EMPTY,
            "在主窗口启动实例后可从这里直达",
            false,
            None::<&str>,
        )?);
    }

    let start_all = MenuItem::with_id(
        app,
        MENU_START_ALL,
        "启动全部实例",
        total > running.len(),
        None::<&str>,
    )?;
    let stop_all = MenuItem::with_id(
        app,
        MENU_STOP_ALL,
        "停止全部实例",
        !running.is_empty(),
        None::<&str>,
    )?;

    let open_data_dir =
        MenuItem::with_id(app, MENU_OPEN_DATA_DIR, "打开数据目录", true, None::<&str>)?;
    let open_log = MenuItem::with_id(app, MENU_OPEN_LOG, "打开运行日志", true, None::<&str>)?;
    let check_update = MenuItem::with_id(
        app,
        MENU_CHECK_UPDATE,
        "检查更新…",
        true,
        Some("CmdOrCtrl+U"),
    )?;
    let open_settings =
        MenuItem::with_id(app, MENU_OPEN_SETTINGS, "打开设置…", true, None::<&str>)?;
    // 无加速键：CmdOrCtrl+R 与前端 Webview 的 Cmd+R 刷新冲突，误触会重启整个启动器，故不设加速键。
    let restart = MenuItem::with_id(app, MENU_RESTART, "重启启动器", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出启动器", true, Some("CmdOrCtrl+Q"))?;

    let sep_top = PredefinedMenuItem::separator(app)?;
    let sep_running = PredefinedMenuItem::separator(app)?;
    let sep_tools = PredefinedMenuItem::separator(app)?;
    let sep_prefs = PredefinedMenuItem::separator(app)?;
    let sep_quit = PredefinedMenuItem::separator(app)?;

    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![
        &status_header,
        &sep_top,
        &open_launcher,
        &quick_sub,
        &sep_running,
    ];
    if let Some(sub) = &running_sub {
        items.push(sub);
    }
    if let Some(hint) = &empty_hint {
        items.push(hint);
    }
    items.push(&start_all);
    items.push(&stop_all);
    items.push(&sep_tools);
    items.push(&open_data_dir);
    items.push(&open_log);
    items.push(&sep_prefs);
    items.push(&check_update);
    items.push(&open_settings);
    items.push(&restart);
    items.push(&sep_quit);
    items.push(&quit);

    Menu::with_items(app, &items)
}

fn show_launcher(app: &AppHandle) {
    crate::windows::show_or_create_main(app);
}

/// Canonical frontend navigation event (t4 `shortcuts.ts` listens to this
/// name and resolves the route payload via `resolveMenuRoute`).
const EVT_MENU_NAVIGATE: &str = "menu-navigate";

/// Shows the main window and asks the frontend to navigate to `route` via
/// the single canonical `menu-navigate` event (M3 收敛：导航只走该规范事件）。
fn show_and_navigate(app: &AppHandle, route: &str) {
    show_launcher(app);
    let _ = app.emit(EVT_MENU_NAVIGATE, route.to_string());
}

/// Starts every configured but currently stopped instance with its
/// last-used (falling back to default, then "web") profile.
/// Reuses `process::start_instance_process`; never touches config files.
fn start_all_instances(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let pending: Vec<(String, String)> = {
            let running = state.running.lock().await;
            let cfg = state.config.lock().unwrap();
            cfg.instances
                .iter()
                .filter(|i| !running.contains_key(&i.id))
                .map(|i| {
                    let profile = i
                        .last_profile
                        .clone()
                        .or_else(|| i.default_profile.clone())
                        .unwrap_or_else(|| "web".to_string());
                    (i.id.clone(), profile)
                })
                .collect()
        };
        if pending.is_empty() {
            crate::log_info!("托盘：全部实例已在运行，无需启动");
            return;
        }
        for (id, profile) in pending {
            if let Err(e) = process::start_instance_process(&app, &state, &id, &profile).await {
                crate::log_warn!("托盘启动全部：实例 {id} 启动失败: {e}");
            }
        }
    });
}

fn stop_all_instances(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let ids: Vec<String> = state.running.lock().await.keys().cloned().collect();
        for id in ids {
            let _ = process::stop_instance_process(&app, &state, &id).await;
        }
    });
}

fn launcher_data_dir(app: &AppHandle) -> std::path::PathBuf {
    if let Some(state) = app.try_state::<AppState>() {
        return state.data_dir.clone();
    }
    app.path().app_data_dir().unwrap_or_default()
}

/// Opens the launcher data directory in the file manager.
/// Mirrors `commands::open_launcher_directory` without adding a command.
fn open_data_dir(app: &AppHandle) {
    let dir = launcher_data_dir(app);
    if dir.as_os_str().is_empty() {
        return;
    }
    if !dir.is_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    crate::log_info!("托盘：在文件管理器中打开数据目录 {}", dir.display());
    if let Err(e) = open::that(&dir) {
        crate::log_warn!("打开数据目录失败: {e}");
    }
}

/// Reveals `<data_dir>/logs/latest.log` in Finder (or opens the log
/// directory when the file does not exist yet). Mirrors
/// `commands::open_launcher_log` without adding a command.
fn open_launcher_log(app: &AppHandle) {
    let log_dir = launcher_data_dir(app).join("logs");
    if log_dir.as_os_str().is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(&log_dir);
    let latest = log_dir.join("latest.log");
    if latest.exists() {
        crate::log_info!("托盘：定位运行日志 {}", latest.display());
        #[cfg(target_os = "macos")]
        {
            if std::process::Command::new("open")
                .arg("-R")
                .arg(&latest)
                .spawn()
                .and_then(|mut c| c.wait())
                .is_ok()
            {
                return;
            }
            crate::log_warn!("open -R 失败，改用默认打开方式");
        }
        if let Err(e) = open::that(&latest) {
            crate::log_warn!("打开运行日志失败: {e}");
        }
    } else {
        crate::log_info!("运行日志不存在，打开日志目录 {}", log_dir.display());
        if let Err(e) = open::that(&log_dir) {
            crate::log_warn!("打开日志目录失败: {e}");
        }
    }
}

/// Checks the dev channel for a newer launcher release: opens the release
/// page when an update exists, otherwise shows the launcher. Failures only
/// log and show the launcher so the tray never looks dead.
fn check_update_from_tray(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match crate::update::check_launcher_update(Some("dev".to_string())).await {
            Ok(info) if !info.up_to_date => {
                if let Some(url) = info.url.as_deref() {
                    crate::log_info!(
                        "托盘：发现新版本 {}，在浏览器中打开 {url}",
                        info.latest.unwrap_or_default()
                    );
                    if open::that(url).is_ok() {
                        return;
                    }
                }
                show_launcher(&app);
            }
            Ok(_) => {
                crate::log_info!("托盘：已是最新版本");
                show_launcher(&app);
            }
            Err(e) => {
                crate::log_warn!("托盘检查更新失败: {e}");
                show_launcher(&app);
            }
        }
    });
}

fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    process::kill_all(&state);
    app.exit(0);
}

fn open_instance_from_tray(app: &AppHandle, instance_id: &str) {
    let app = app.clone();
    let id = instance_id.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let url = state.running.lock().await.get(&id).map(|r| r.url.clone());
        let url = match url {
            Some(Some(u)) => u,
            _ => return,
        };
        // 与 open_external 保持一致：先裁空白再做 http(s) 校验。
        let url = url.trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            crate::log_warn!("托盘打开实例拒绝非 http(s) 链接: {url}");
            return;
        }
        crate::log_info!("托盘在系统浏览器打开实例 {id}：{url}");
        if let Err(e) = open::that(&url) {
            crate::log_warn!("托盘打开实例失败: {e}");
        }
    });
}

fn stop_instance_from_tray(app: &AppHandle, instance_id: &str) {
    let app = app.clone();
    let id = instance_id.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _ = process::stop_instance_process(&app, &state, &id).await;
    });
}

fn handle_double_click(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        // 仅当唯一运行实例时直达其页面（系统浏览器），否则显示主窗口。
        let running = state.running.lock().await;
        let target_url = if running.len() == 1 {
            running.values().next().and_then(|e| e.url.clone())
        } else {
            None
        };
        drop(running);

        if let Some(url) = target_url {
            // 与 open_external 保持一致：先裁空白再做 http(s) 校验。
            let url = url.trim().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                crate::log_warn!("托盘双击拒绝非 http(s) 链接: {url}");
                show_launcher(&app);
                return;
            }
            crate::log_info!("托盘双击在系统浏览器打开 {url}");
            if open::that(&url).is_err() {
                show_launcher(&app);
            }
        } else {
            show_launcher(&app);
        }
    });
}
