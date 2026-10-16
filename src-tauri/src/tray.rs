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
/// 运行中子菜单里两个动作的前缀，与 `open::` / `stop::` 的 id 前缀配套。
const MENU_OPEN_LABEL: &str = "打开：";
const MENU_STOP_LABEL: &str = "停止：";
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

/// One running instance as the tray sees it: the record it belongs to, the name
/// the user gave it, the machine it runs on, and the profile it runs.
///
/// The machine is part of the item, not of the menu: this menu is one list of
/// every host's instances (ADR-0006 §4), and two machines may both run an
/// instance called `web` — the name by itself would not say which one.
struct RunningItem {
    id: String,
    name: String,
    host: String,
    profile: String,
}

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
            } else if id == MENU_QUICK_VERSIONS {
                show_and_navigate(app, "/library?tab=versions");
            } else if id == MENU_QUICK_HOMES {
                show_and_navigate(app, "/library?tab=homes");
            } else if id == MENU_QUICK_PROFILES {
                show_and_navigate(app, "/library?tab=profiles");
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
    running_items(&cfg, &running)
}

/// Every running instance, with the machine it runs on — the facts the tray
/// menu and its actions are built from.
///
/// The host comes from the instance's own record: the running table says a
/// process is up, the config says what the user calls it and where it runs. An
/// entry whose record is gone still gets a line — it really is running — and is
/// named by its id.
///
/// The order is the menu's: by machine, then by name. Reading a HashMap in
/// place would reshuffle the menu every time an instance starts or stops.
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

/// How the tray spells one host. A record whose host has no name of its own
/// falls back to its id, so no line reads as if it had no machine at all.
fn host_label(cfg: &Config, host_id: &str) -> String {
    cfg.hosts
        .iter()
        .find(|h| h.id == host_id)
        .map(|h| h.display_name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| host_id.to_string())
}

/// How the tray names one instance when it has to talk about it: the name the
/// user gave it and the machine it is on — never the name alone, because two
/// machines may both run an instance called `web`.
fn instance_label(name: &str, host: &str) -> String {
    format!("{name}（{host}）")
}

/// One action line in the running submenu: what the click does, the instance's
/// name, the machine it runs on, and the profile it runs.
fn action_label(action: &str, name: &str, host: &str, profile: &str) -> String {
    format!("{action}{name}（{host} · {profile}）")
}

/// Which instance a double click opens straight away: the only one running, or
/// none.
///
/// 「Only one」 counts every machine's instances together. The menu is a single
/// list and a double click has no host to narrow it with, so two machines with
/// one instance each is not one instance — that shows the window instead.
fn sole_running(items: &[RunningItem]) -> Option<&str> {
    match items {
        [only] => Some(only.id.as_str()),
        _ => None,
    }
}

/// What one stop that did not happen tells the user. A stop fails for real
/// reasons now — a pid that is no longer that process, a machine that does not
/// answer, a process that will not go — and the tray has no page to say them on,
/// so they come out here and go into a dialog.
fn stop_failure_note(label: &str, reason: &str) -> String {
    format!("停止实例「{label}」失败：{reason}")
}

/// The same for a stop-all: one list naming every instance that did not stop,
/// or nothing at all when they all did. One dialog for the whole action —
/// a stream of them is worse than a list.
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

/// What a start-all says when it did not do everything it was asked to. Two
/// things can hold an instance back and they are not the same news: a start that
/// ran and failed, with the reason the backend gave, and an instance left alone
/// because its machine could not be reached — never attempted, since nothing is
/// known about what already runs there (ADR-0007). `None` when every instance it
/// set out for did start.
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

/// One line per instance that did not go through, naming it and repeating the
/// reason it gave. Both bulk actions say their failures this way, so the two
/// dialogs cannot end up listing them differently.
fn failure_lines(failures: &[(String, String)]) -> String {
    failures
        .iter()
        .map(|(label, reason)| format!("· {label}：{reason}"))
        .collect::<Vec<_>>()
        .join("\n")
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
    let quick_home = MenuItem::with_id(app, MENU_QUICK_HOME, "实例", true, None::<&str>)?;
    // DSH 版本 / DSH_HOME / Profile 都是资源库的标签页，点进去直达对应那一页。
    // 插件管理不占格：它属于运行中的实例，入口在实例行展开的卡片里。
    let quick_versions =
        MenuItem::with_id(app, MENU_QUICK_VERSIONS, "DSH 版本", true, None::<&str>)?;
    let quick_homes = MenuItem::with_id(app, MENU_QUICK_HOMES, "DSH_HOME", true, None::<&str>)?;
    let quick_profiles =
        MenuItem::with_id(app, MENU_QUICK_PROFILES, "Profile", true, None::<&str>)?;
    let quick_refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
        vec![&quick_home, &quick_homes, &quick_profiles, &quick_versions];
    let quick_sub = Submenu::with_id_and_items(app, MENU_QUICK_SUB, "快捷入口", true, &quick_refs)?;

    // Running profiles live in a second-level submenu: each instance gets an
    // "open window" and a "stop" entry. Every line carries the machine the
    // instance runs on, so two machines' instances of the same name stay apart.
    // Empty state is a disabled hint; the status header above already says
    // "暂无运行中".
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
///
/// A machine the launcher cannot reach is **skipped, not attempted**: nothing is
/// known about what already runs there, and the click that starts one anyway is
/// how a second DSH lands on a server (ADR-0007). The tray has no page to say
/// that on, so it says it in one dialog — the same rule the board's 全部启动
/// follows (`instancesBulk.ts`), and the same reason that button reports skips.
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

/// One instance a start-all will attempt: what to start, with which profile,
/// and how the tray names it if the start fails.
struct StartTarget {
    id: String,
    profile: String,
    label: String,
}

/// The one rule behind the tray's 全部启动: which stopped instances it attempts,
/// and which it leaves alone.
///
/// Every machine is in scope — the menu has no host dimension — but a machine
/// the launcher cannot reach is **skipped, not attempted**: nothing is known
/// about what already runs there, and the click that starts one anyway is how a
/// second DSH lands on a server (ADR-0007). The skips come back with the
/// attempts, so the caller has to say them out loud rather than silently do
/// less. The board's 全部启动 follows the same rule (`instancesBulk.ts`), which
/// is why this takes the reachability judgement as an argument: the one answer
/// to 「那台机器现在能问吗」 lives on [`AppState::host_unreachable_reason`].
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
        // Every machine's running instances: this menu has no host dimension —
        // it is one list (ADR-0006 §3) — so 「停止全部」 means all of it. A stop
        // that does not happen is named with the reason it gave: the tray has
        // no page to show it on.
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

/// Opens one running instance in the system browser, from the tray.
///
/// It goes through the same rule the instance row does
/// ([`crate::process::reachable_url_for`]): a remote instance is opened at the
/// address **this Mac** can reach it at, through the launcher's port forward,
/// and never at the address DSH printed on its own machine — that address means
/// a different service here, and the token in it would be handed to whatever
/// answers. The tray has no page to report a failure on, so a failure says why
/// in a dialog instead of doing nothing.
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
        // 与 open_external 保持一致：先裁空白再做 http(s) 校验。
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

/// Says one thing to the user from a place with no page to say it on. The tray
/// menu has no window, so a failure there would otherwise be silence.
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
            // The reasons are real ones now — a reused pid, an unreachable
            // machine, a process that will not go — and silence here is the one
            // outcome that misleads.
            crate::log_warn!("托盘停止实例 {id} 失败：{reason}");
            report_to_user(&app, &stop_failure_note(&label, &reason));
        }
    });
}

fn handle_double_click(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        // 仅当唯一运行实例时直达其页面（系统浏览器），否则显示主窗口。
        // 「唯一」把每台机器的实例算在一起：菜单是一份跨主机的清单。
        let target = sole_running(&running_snapshot(&app).await).map(str::to_string);

        // The same rule the tray's own 「打开」 uses, so a remote instance is
        // reached through the launcher's forward here too. A failure falls through
        // to the window — what a double click does when there is nothing to open
        // — but it is said in the log rather than swallowed.
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

    /// 全部启动跳过不可达主机上的实例，并把跳过的报出来：那种状态下启动器说
    /// 不出那台机器上是不是已经在跑，而在服务器上起第二个 DSH 正是 ADR-0007
    /// 存在的理由。页面的「全部启动」用的是同一条规则（`instancesBulk.ts`）。
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

    /// 已经在运行的实例不在「全部启动」的范围里：这不是跳过，是没什么可做。
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

    /// 两台机器上的同名实例不是同一行：名字后面必须跟着它跑在哪台机器上
    /// （ADR-0006 §4），否则「打开 web」在两台机器上都叫 web 时无处可选。
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

    /// 双击的「只有一个运行实例时直达」数的是**所有机器**的运行实例：菜单
    /// 是一份跨主机的清单，双击也没有主机可以缩小范围。
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

    /// 停止失败现在有真原因（pid 已被复用、主机不可达、进程不肯走），托盘
    /// 没有页面可以显示它们，所以原因必须原样进对话框。
    #[test]
    fn a_stop_that_failed_names_the_instance_and_the_reason() {
        let note = stop_failure_note("web（阿里云服务器）", "主机不可达：连接超时");
        assert!(
            note.contains("web（阿里云服务器）"),
            "要说清是哪一台：{note}"
        );
        assert!(note.contains("主机不可达：连接超时"), "要带上原因：{note}");
    }

    /// 停止全部失败时给一条清单而不是一串弹窗：都停下来了就什么都不说。
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

        // 只有一条失败时不必摆成清单，直接说那一条。
        let one = vec![(
            "web（本机）".to_string(),
            "停止实例超时（进程未响应终止信号）".to_string(),
        )];
        assert_eq!(
            stop_all_failure_note(&one).as_deref(),
            Some("停止实例「web（本机）」失败：停止实例超时（进程未响应终止信号）")
        );
    }

    /// 启动全部的两件事分开说：跑了没成（带后端那句原因）与根本没动的
    /// （机器连不上，跳过）。都成了就什么都不说——干净的一轮不需要弹窗。
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

        // 只有跳过的：没有失败也照样要说，否则用户看到的是一个没反应的按钮。
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
