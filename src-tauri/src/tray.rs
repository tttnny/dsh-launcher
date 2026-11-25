use crate::config::{Config, LOCAL_HOST_ID};
use crate::process::{self, RunningInstance};
use crate::AppState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const MENU_OPEN_LAUNCHER: &str = "open-launcher";
const MENU_QUIT: &str = "quit";
const MENU_RUNNING_SUB: &str = "running-profiles";
const MENU_OPEN_PREFIX: &str = "open::";
const MENU_STOP_PREFIX: &str = "stop::";
const MENU_OPEN_LABEL: &str = "打开：";
const MENU_STOP_LABEL: &str = "停止：";
const MENU_STATUS: &str = "status-header";
const MENU_EMPTY: &str = "empty-hint";
const MENU_QUICK_SUB: &str = "quick-entries";
const MENU_QUICK_HOME: &str = "quick::home";
const MENU_QUICK_INSTALL: &str = "quick::install";
const MENU_QUICK_DATA: &str = "quick::data";
const MENU_QUICK_MACHINES: &str = "quick::machines";
const MENU_START_ALL: &str = "start-all";
const MENU_STOP_ALL: &str = "stop-all";
const MENU_OPEN_DATA_DIR: &str = "open-data-dir";
const MENU_OPEN_LOG: &str = "open-log";
const MENU_CHECK_UPDATE: &str = "check-update";
const MENU_OPEN_SETTINGS: &str = "open-settings";
const MENU_RESTART: &str = "restart";

struct RunningItem {
    id: String,
    name: String,
    host: String,
    profile: String,
}

const LEFT_CLICK_DEBOUNCE_MS: u64 = 250;
static LAST_LEFT_UP_MS: AtomicU64 = AtomicU64::new(0);
static LAST_DOUBLE_CLICK_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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
            } else if id == MENU_QUICK_INSTALL {
                show_and_navigate(app, "/install");
            } else if id == MENU_QUICK_DATA {
                show_and_navigate(app, "/data");
            } else if id == MENU_QUICK_MACHINES {
                show_and_navigate(app, "/machines");
            } else if id == MENU_START_ALL {
                start_all_instances(app);
            } else if id == MENU_STOP_ALL {
                stop_all_instances(app);
            } else if id == MENU_OPEN_DATA_DIR {
                open_data_dir(app);
            } else if id == MENU_OPEN_LOG {
                open_launcher_log(app);
            } else if id == MENU_CHECK_UPDATE {
                crate::app_menu::request_update_check(app);
            } else if id == MENU_OPEN_SETTINGS {
                show_and_navigate(app, "/settings");
            } else if id == MENU_RESTART {
                app.restart();
            } else if id == MENU_QUIT {
                quit(app);
            } else if let Some(instance_id) = id.strip_prefix(MENU_OPEN_PREFIX) {
                open_instance_from_tray(app, instance_id);
            } else if let Some(instance_id) = id.strip_prefix(MENU_STOP_PREFIX) {
                stop_instance_from_tray(app, instance_id);
            }
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                let stamp = now_ms();
                LAST_LEFT_UP_MS.store(stamp, Ordering::SeqCst);
                let handle = tray.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(LEFT_CLICK_DEBOUNCE_MS))
                        .await;
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
            } => {}
            TrayIconEvent::DoubleClick { .. } => {
                LAST_DOUBLE_CLICK_MS.store(now_ms(), Ordering::SeqCst);
                handle_double_click(tray.app_handle());
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

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
    running_items(&cfg, &running)
}

fn running_items(cfg: &Config, running: &HashMap<String, RunningInstance>) -> Vec<RunningItem> {
    let mut items: Vec<RunningItem> = running
        .iter()
        .map(|(id, entry)| {
            let inst = cfg.instances.iter().find(|i| i.id == *id);
            let host_id = inst
                .map(|i| i.host_id.trim().to_string())
                .filter(|host| !host.is_empty())
                .or_else(|| entry.remote.as_ref().map(|run| run.host_id.clone()))
                .unwrap_or_else(|| LOCAL_HOST_ID.to_string());
            RunningItem {
                id: id.clone(),
                name: inst.map(|i| i.name.clone()).unwrap_or_else(|| id.clone()),
                host: host_label(cfg, &host_id),
                profile: entry.profile.clone(),
            }
        })
        .collect();
    items.sort_by(|a, b| a.host.cmp(&b.host).then_with(|| a.name.cmp(&b.name)));
    items
}

fn host_label(cfg: &Config, host_id: &str) -> String {
    cfg.hosts
        .iter()
        .find(|h| h.id == host_id)
        .map(|h| h.display_name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| host_id.to_string())
}

fn instance_label(name: &str, host: &str) -> String {
    format!("{name}（{host}）")
}

fn action_label(action: &str, name: &str, host: &str, profile: &str) -> String {
    format!("{action}{name}（{host} · {profile}）")
}

fn sole_running(items: &[RunningItem]) -> Option<&str> {
    match items {
        [only] => Some(only.id.as_str()),
        _ => None,
    }
}

fn stop_failure_note(label: &str, reason: &str) -> String {
    format!("停止实例「{label}」失败：{reason}")
}

fn stop_all_failure_note(failures: &[(String, String)]) -> Option<String> {
    match failures {
        [] => None,
        [(label, reason)] => Some(stop_failure_note(label, reason)),
        many => Some(format!(
            "有 {} 个实例没能停止：\n{}",
            many.len(),
            failure_lines(many)
        )),
    }
}

fn start_all_report(failures: &[(String, String)], skipped: &[String]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match failures {
        [] => {}
        [(label, reason)] => parts.push(format!("启动实例「{label}」失败：{reason}")),
        many => parts.push(format!(
            "有 {} 个实例没能启动：\n{}",
            many.len(),
            failure_lines(many)
        )),
    }
    if !skipped.is_empty() {
        parts.push(format!(
            "跳过 {} 个实例：它们所在的机器现在连不上，启动器不知道那边是不是已经在跑，所以没有动它们。\n{}",
            skipped.len(),
            skipped
                .iter()
                .map(|label| format!("· {label}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

fn failure_lines(failures: &[(String, String)]) -> String {
    failures
        .iter()
        .map(|(label, reason)| format!("· {label}：{reason}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_menu(app: &AppHandle, running: &[RunningItem]) -> tauri::Result<Menu<tauri::Wry>> {
    let total: usize = app
        .try_state::<AppState>()
        .map(|s| s.config.lock().unwrap().instances.len())
        .unwrap_or(0);

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

    let open_launcher = MenuItem::with_id(
        app,
        MENU_OPEN_LAUNCHER,
        "显示主窗口",
        true,
        Some("CmdOrCtrl+O"),
    )?;

    let quick_home = MenuItem::with_id(app, MENU_QUICK_HOME, "实例", true, None::<&str>)?;
    let quick_install = MenuItem::with_id(app, MENU_QUICK_INSTALL, "安装", true, None::<&str>)?;
    let quick_data = MenuItem::with_id(app, MENU_QUICK_DATA, "数据", true, None::<&str>)?;
    let quick_machines = MenuItem::with_id(app, MENU_QUICK_MACHINES, "机器", true, None::<&str>)?;
    let quick_refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
        vec![&quick_home, &quick_install, &quick_data, &quick_machines];
    let quick_sub = Submenu::with_id_and_items(app, MENU_QUICK_SUB, "快捷入口", true, &quick_refs)?;

    let mut owned: Vec<MenuItem<tauri::Wry>> = Vec::new();
    let mut running_sub = None;
    let mut empty_hint = None;
    if !running.is_empty() {
        for item in running {
            owned.push(MenuItem::with_id(
                app,
                format!("{MENU_OPEN_PREFIX}{}", item.id),
                action_label(MENU_OPEN_LABEL, &item.name, &item.host, &item.profile),
                true,
                None::<&str>,
            )?);
            owned.push(MenuItem::with_id(
                app,
                format!("{MENU_STOP_PREFIX}{}", item.id),
                action_label(MENU_STOP_LABEL, &item.name, &item.host, &item.profile),
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
            format!("运行中的实例 ({})", running.len()),
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

const EVT_MENU_NAVIGATE: &str = "menu-navigate";

fn show_and_navigate(app: &AppHandle, route: &str) {
    show_launcher(app);
    let _ = app.emit(EVT_MENU_NAVIGATE, route.to_string());
}

fn start_all_instances(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let (targets, skipped) = {
            let running = state.running.lock().await;
            let cfg = state.config.lock().unwrap();
            bulk_start_targets(&cfg, &running, |host_id| {
                state.host_unreachable_reason(host_id).is_some()
            })
        };
        if targets.is_empty() && skipped.is_empty() {
            crate::log_info!("托盘：全部实例已在运行，无需启动");
            return;
        }
        let mut failures: Vec<(String, String)> = Vec::new();
        for target in targets {
            let started =
                process::start_instance_process(&app, &state, &target.id, &target.profile).await;
            if let Err(reason) = started {
                crate::log_warn!("托盘启动全部：实例 {} 启动失败: {reason}", target.id);
                failures.push((target.label, reason));
            }
        }
        if let Some(note) = start_all_report(&failures, &skipped) {
            report_to_user(&app, &note);
        }
    });
}

struct StartTarget {
    id: String,
    profile: String,
    label: String,
}

fn bulk_start_targets(
    cfg: &Config,
    running: &HashMap<String, RunningInstance>,
    unreachable: impl Fn(&str) -> bool,
) -> (Vec<StartTarget>, Vec<String>) {
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for inst in cfg
        .instances
        .iter()
        .filter(|i| !running.contains_key(&i.id))
    {
        let host_id = if inst.host_id.trim().is_empty() {
            LOCAL_HOST_ID
        } else {
            inst.host_id.trim()
        };
        let label = instance_label(&inst.name, &host_label(cfg, host_id));
        if unreachable(host_id) {
            skipped.push(label);
            continue;
        }
        targets.push(StartTarget {
            id: inst.id.clone(),
            profile: inst
                .last_profile
                .clone()
                .or_else(|| inst.default_profile.clone())
                .unwrap_or_else(|| "web".to_string()),
            label,
        });
    }
    (targets, skipped)
}

fn stop_all_instances(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut failures: Vec<(String, String)> = Vec::new();
        for item in running_snapshot(&app).await {
            if let Err(reason) = process::stop_instance_process(&app, &state, &item.id).await {
                crate::log_warn!("托盘停止全部：实例 {} 停止失败：{reason}", item.id);
                failures.push((instance_label(&item.name, &item.host), reason));
            }
        }
        if let Some(note) = stop_all_failure_note(&failures) {
            report_to_user(&app, &note);
        }
    });
}

fn launcher_data_dir(app: &AppHandle) -> std::path::PathBuf {
    if let Some(state) = app.try_state::<AppState>() {
        return state.data_dir.clone();
    }
    app.path().app_data_dir().unwrap_or_default()
}

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
        let url = match crate::process::reachable_url_for(&state, &id).await {
            Ok(url) => url,
            Err(reason) => {
                crate::log_warn!("托盘打开实例 {id} 失败：{reason}");
                report_to_user(&app, &reason);
                return;
            }
        };
        let url = url.trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            crate::log_warn!("托盘打开实例拒绝非 http(s) 链接: {url}");
            return;
        }
        crate::log_info!("托盘在系统浏览器打开实例 {id}：{url}");
        if let Err(e) = open::that(&url) {
            crate::log_warn!("托盘打开实例失败: {e}");
            report_to_user(&app, &format!("打开浏览器失败：{e}"));
        }
    });
}

fn report_to_user(app: &AppHandle, message: &str) {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .message(message.to_string())
        .title("dsh-launcher")
        .show(|_| {});
}

fn stop_instance_from_tray(app: &AppHandle, instance_id: &str) {
    let app = app.clone();
    let id = instance_id.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let label = running_snapshot(&app)
            .await
            .iter()
            .find(|item| item.id == id)
            .map(|item| instance_label(&item.name, &item.host))
            .unwrap_or_else(|| id.clone());
        if let Err(reason) = process::stop_instance_process(&app, &state, &id).await {
            crate::log_warn!("托盘停止实例 {id} 失败：{reason}");
            report_to_user(&app, &stop_failure_note(&label, &reason));
        }
    });
}

fn handle_double_click(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let target = sole_running(&running_snapshot(&app).await).map(str::to_string);

        let url = match target {
            Some(id) => match crate::process::reachable_url_for(&state, &id).await {
                Ok(url) => Some(url),
                Err(reason) => {
                    crate::log_warn!("托盘双击打开实例 {id} 失败：{reason}");
                    None
                }
            },
            None => None,
        };

        if let Some(url) = url {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DshInstance, Host};

    fn server_host() -> Host {
        Host {
            id: "host-server".into(),
            display_name: "阿里云服务器".into(),
            ssh_target: "zjq_aliyun".into(),
            registry: None,
            extra_env: Default::default(),
            data_dir: Some(std::path::PathBuf::from(
                "/home/demo/.local/share/dsh-launcher",
            )),
            builtin: false,
            toolchain: Default::default(),
            toolchain_overrides: Default::default(),
        }
    }

    fn instance(id: &str, name: &str, host_id: &str) -> DshInstance {
        DshInstance {
            id: id.to_string(),
            name: name.to_string(),
            version_id: "v1".into(),
            home_id: "h1".into(),
            host_id: host_id.to_string(),
            env_overrides: Default::default(),
            default_profile: None,
            last_profile: None,
            icon: None,
            port: None,
        }
    }

    fn entry(profile: &str) -> RunningInstance {
        RunningInstance {
            kill: std::sync::Arc::new(tokio::sync::Notify::new()),
            profile: profile.to_string(),
            url: None,
            adopted: None,
            remote: None,
        }
    }

    fn config(instances: Vec<DshInstance>) -> Config {
        Config {
            homes: Vec::new(),
            versions: Vec::new(),
            instances,
            settings: Default::default(),
            hosts: vec![Host::local(), server_host()],
        }
    }

    fn item(id: &str, name: &str, host: &str) -> RunningItem {
        RunningItem {
            id: id.to_string(),
            name: name.to_string(),
            host: host.to_string(),
            profile: "web".into(),
        }
    }

    #[test]
    fn a_start_all_skips_the_instances_of_machines_it_cannot_reach() {
        let cfg = config(vec![
            instance("i-local", "web", LOCAL_HOST_ID),
            instance("i-server", "api", "host-server"),
        ]);
        let running = HashMap::new();

        let (targets, skipped) =
            bulk_start_targets(&cfg, &running, |host_id| host_id == "host-server");

        assert_eq!(
            targets
                .iter()
                .map(|t| (t.id.as_str(), t.profile.as_str(), t.label.as_str()))
                .collect::<Vec<_>>(),
            vec![("i-local", "web", "web（本机）")],
            "到得了的机器照常启动，带上它自己选好的 Profile"
        );
        assert_eq!(
            skipped,
            vec!["api（阿里云服务器）".to_string()],
            "跳过的要带名字与机器名回来，调用方才说得出来"
        );
    }

    #[test]
    fn a_start_all_leaves_running_instances_alone() {
        let cfg = config(vec![
            instance("i-local", "web", LOCAL_HOST_ID),
            instance("i-local-2", "api", LOCAL_HOST_ID),
        ]);
        let mut running = HashMap::new();
        running.insert("i-local".to_string(), entry("web"));

        let (targets, skipped) = bulk_start_targets(&cfg, &running, |_| false);

        assert_eq!(
            targets.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["i-local-2"],
            "只启动停着的那些"
        );
        assert!(skipped.is_empty(), "本机的问题不是「跳过」：{skipped:?}");
    }

    #[test]
    fn every_running_instance_says_which_machine_it_runs_on() {
        let cfg = config(vec![
            instance("i-local", "web", LOCAL_HOST_ID),
            instance("i-server", "web", "host-server"),
        ]);
        let mut running = HashMap::new();
        running.insert("i-local".to_string(), entry("web"));
        running.insert("i-server".to_string(), entry("default"));

        let items = running_items(&cfg, &running);
        let lines: Vec<String> = items
            .iter()
            .map(|i| action_label(MENU_OPEN_LABEL, &i.name, &i.host, &i.profile))
            .collect();

        assert_eq!(
            lines,
            vec![
                "打开：web（本机 · web）",
                "打开：web（阿里云服务器 · default）",
            ],
            "菜单按主机名与实例名排序，两台机器的同名实例各占一行"
        );
    }

    #[test]
    fn a_double_click_goes_straight_to_the_only_running_instance_anywhere() {
        let only_local = vec![item("i-local", "web", "本机")];
        assert_eq!(sole_running(&only_local), Some("i-local"));

        let only_remote = vec![item("i-server", "web", "阿里云服务器")];
        assert_eq!(sole_running(&only_remote), Some("i-server"));

        let one_per_machine = vec![
            item("i-local", "web", "本机"),
            item("i-server", "web", "阿里云服务器"),
        ];
        assert_eq!(
            sole_running(&one_per_machine),
            None,
            "两台机器各一个不是「只有一个」：双击该显示主窗口"
        );

        assert_eq!(sole_running(&[]), None);
    }

    #[test]
    fn a_stop_that_failed_names_the_instance_and_the_reason() {
        let note = stop_failure_note("web（阿里云服务器）", "主机不可达：连接超时");
        assert!(
            note.contains("web（阿里云服务器）"),
            "要说清是哪一台：{note}"
        );
        assert!(note.contains("主机不可达：连接超时"), "要带上原因：{note}");
    }

    #[test]
    fn stop_all_reports_every_instance_that_did_not_stop_in_one_note() {
        assert!(stop_all_failure_note(&[]).is_none());

        let failures = vec![
            (
                "web（本机）".to_string(),
                "停止实例超时（进程未响应终止信号）".to_string(),
            ),
            (
                "api（阿里云服务器）".to_string(),
                "主机不可达：连接超时".to_string(),
            ),
        ];
        let note = stop_all_failure_note(&failures).expect("有失败就要说出来");
        for said in [
            "web（本机）",
            "停止实例超时（进程未响应终止信号）",
            "api（阿里云服务器）",
            "主机不可达：连接超时",
        ] {
            assert!(note.contains(said), "清单里少了「{said}」：{note}");
        }

        let one = vec![(
            "web（本机）".to_string(),
            "停止实例超时（进程未响应终止信号）".to_string(),
        )];
        assert_eq!(
            stop_all_failure_note(&one).as_deref(),
            Some("停止实例「web（本机）」失败：停止实例超时（进程未响应终止信号）")
        );
    }

    #[test]
    fn a_start_all_says_what_it_did_not_do() {
        assert!(start_all_report(&[], &[]).is_none(), "都起来了就别弹窗");

        let failures = vec![("web（本机）".to_string(), "端口 3000 被占用".to_string())];
        let note = start_all_report(&failures, &["api（阿里云服务器）".to_string()])
            .expect("没做完的事要说出来");
        assert!(note.contains("web（本机）"), "{note}");
        assert!(
            note.contains("端口 3000 被占用"),
            "失败要带后端那句原因：{note}"
        );
        assert!(
            note.contains("api（阿里云服务器）"),
            "跳过的那台要点名：{note}"
        );
        assert!(note.contains("跳过"), "跳过与失败不是一回事：{note}");

        let only_skipped =
            start_all_report(&[], &["api（阿里云服务器）".to_string()]).expect("跳过要说");
        assert!(
            only_skipped.contains("api（阿里云服务器）"),
            "{only_skipped}"
        );
        assert!(
            !only_skipped.contains("失败"),
            "没失败就别提失败：{only_skipped}"
        );
    }
}
