use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
pub struct FullDiskAccessStatus {
    pub supported: bool,
    pub granted: bool,
    pub app_path: Option<String>,
    pub error: Option<String>,
}

fn app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        })
        .map(Path::to_path_buf)
}

#[cfg(target_os = "macos")]
fn probe_full_disk_access() -> (bool, Option<String>) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return (false, Some("找不到当前用户的主目录".to_string()));
    };
    let probes = [
        home.join("Library/Application Support/com.apple.TCC/TCC.db"),
        home.join("Library/Safari/History.db"),
        home.join("Library/Messages/chat.db"),
    ];
    let mut denied = false;
    let mut missing = 0usize;
    let mut unexpected = None;
    for path in probes {
        match std::fs::File::open(&path) {
            Ok(_) => return (true, None),
            Err(err)
                if err.kind() == std::io::ErrorKind::PermissionDenied
                    || err.raw_os_error() == Some(libc::EPERM) =>
            {
                denied = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => missing += 1,
            Err(err) => {
                unexpected = Some(format!("检测完全磁盘访问失败：{err}"));
            }
        }
    }
    if denied {
        (false, None)
    } else if missing == 3 {
        (
            false,
            Some("找不到可以用来判断权限的受保护文件".to_string()),
        )
    } else {
        (false, unexpected)
    }
}

#[cfg(not(target_os = "macos"))]
fn probe_full_disk_access() -> (bool, Option<String>) {
    (false, Some("仅 macOS 需要完全磁盘访问权限".to_string()))
}

#[tauri::command]
pub fn full_disk_access_status() -> FullDiskAccessStatus {
    let app_path = app_bundle_path();
    let (granted, error) = probe_full_disk_access();
    FullDiskAccessStatus {
        supported: cfg!(target_os = "macos"),
        granted,
        app_path: app_path.map(|path| path.to_string_lossy().to_string()),
        error,
    }
}

const SETTINGS_PANES: [&str; 2] = [
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles",
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
];

fn settings_pane_urls() -> [&'static str; 2] {
    let major = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|version| version.trim().split('.').next()?.parse::<u32>().ok());
    match major {
        Some(major) if major < 13 => [SETTINGS_PANES[1], SETTINGS_PANES[0]],
        _ => SETTINGS_PANES,
    }
}

#[tauri::command]
pub fn open_full_disk_access_settings() -> Result<(), String> {
    for url in settings_pane_urls() {
        if open::that(url).is_ok() {
            crate::log_info!("打开系统设置的完全磁盘访问页面");
            return Ok(());
        }
    }
    Err("打开系统设置失败，请从「系统设置 › 隐私与安全性 › 完全磁盘访问」进入".to_string())
}

fn bundle_for_action() -> Result<PathBuf, String> {
    app_bundle_path().ok_or_else(|| {
        "当前不是从安装后的 .app 运行，没有可以拖动的应用包；请使用安装后的启动器操作".to_string()
    })
}

#[tauri::command]
pub async fn show_full_disk_access_drag_panel(app: tauri::AppHandle) -> Result<(), String> {
    let bundle = bundle_for_action()?;
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = sender.send(macos::show_drag_panel(&bundle));
        })
        .map_err(|err| format!("无法打开拖动面板：{err}"))?;
        receiver.await.map_err(|_| "拖动面板没有回应".to_string())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, bundle);
        Err("仅 macOS 支持完全磁盘访问权限".to_string())
    }
}

#[tauri::command]
pub async fn close_full_disk_access_drag_panel(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = sender.send(macos::close_drag_panel());
        })
        .map_err(|err| format!("无法关闭拖动面板：{err}"))?;
        receiver.await.map_err(|_| "拖动面板没有回应".to_string())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

#[tauri::command]
pub async fn reveal_app_bundle(app: tauri::AppHandle) -> Result<(), String> {
    let bundle = bundle_for_action()?;
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = sender.send(macos::reveal(&bundle));
        })
        .map_err(|err| format!("无法在访达中显示：{err}"))?;
        receiver.await.map_err(|_| "访达没有回应".to_string())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, bundle);
        Err("仅 macOS 支持完全磁盘访问权限".to_string())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSColor, NSDragOperation, NSDraggingContext,
        NSDraggingItem, NSDraggingSession, NSDraggingSource, NSFloatingWindowLevel, NSFont,
        NSImage, NSImageScaling, NSImageView, NSPasteboardWriting, NSScreen, NSTextField, NSView,
        NSWindow, NSWindowStyleMask, NSWorkspace,
    };
    use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};
    use std::cell::RefCell;

    thread_local! {
        static PANEL: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
        static DRAG_URL: RefCell<Option<Retained<NSURL>>> = const { RefCell::new(None) };
    }

    define_class!(
        #[unsafe(super(NSImageView))]
        #[thread_kind = MainThreadOnly]
        #[name = "DSHFullDiskAccessDragIcon"]
        struct DragIcon;

        unsafe impl NSObjectProtocol for DragIcon {}

        impl DragIcon {
            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _event: Option<&objc2_app_kit::NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _event: &objc2_app_kit::NSEvent) {}

            #[unsafe(method(mouseDragged:))]
            fn mouse_dragged(&self, event: &objc2_app_kit::NSEvent) {
                let Some(url) = DRAG_URL.with(|slot| slot.borrow().clone()) else {
                    return;
                };
                let Some(image) = self.image() else {
                    return;
                };
                let writer = ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*url);
                let item =
                    NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), writer);
                let contents = unsafe { &*(&*image as *const NSImage as *const AnyObject) };
                unsafe {
                    item.setDraggingFrame_contents(self.bounds(), Some(contents));
                }
                let items = NSArray::from_retained_slice(&[item]);
                let source = ProtocolObject::<dyn NSDraggingSource>::from_ref(self);
                self.beginDraggingSessionWithItems_event_source(&items, event, source);
            }
        }

        unsafe impl NSDraggingSource for DragIcon {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn operation_mask(
                &self,
                _session: &NSDraggingSession,
                _context: NSDraggingContext,
            ) -> NSDragOperation {
                NSDragOperation::Copy
            }
        }
    );

    fn make_label(
        text: &str,
        font: Retained<NSFont>,
        color: Retained<NSColor>,
        frame: NSRect,
    ) -> Retained<NSTextField> {
        let mtm = MainThreadMarker::new().expect("AppKit 只能在主线程使用");
        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
        label.setFrame(frame);
        label.setFont(Some(&font));
        label.setTextColor(Some(&color));
        label
    }

    #[allow(deprecated)]
    pub fn show_drag_panel(bundle: &Path) -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or("拖动面板只能在主线程打开")?;
        let app = NSApplication::sharedApplication(mtm);
        app.activateIgnoringOtherApps(true);

        let already_open = PANEL.with(|slot| match slot.borrow().as_ref() {
            Some(window) => {
                window.makeKeyAndOrderFront(None);
                true
            }
            None => false,
        });
        if already_open {
            return Ok(());
        }

        let path = bundle.to_string_lossy().to_string();
        let ns_path = NSString::from_str(&path);
        let url = NSURL::fileURLWithPath(&ns_path);
        DRAG_URL.with(|slot| *slot.borrow_mut() = Some(url));

        let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(360.0, 118.0));
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                content_rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("完全磁盘访问权限"));
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setLevel(NSFloatingWindowLevel);

        let container = NSView::initWithFrame(mtm.alloc::<NSView>(), content_rect);
        let workspace = NSWorkspace::sharedWorkspace();
        let icon = workspace.iconForFile(&ns_path);
        let icon_view: Retained<DragIcon> = unsafe {
            msg_send![
                mtm.alloc::<DragIcon>(),
                initWithFrame: NSRect::new(NSPoint::new(18.0, 27.0), NSSize::new(64.0, 64.0))
            ]
        };
        icon_view.setImage(Some(&icon));
        icon_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);

        let title = make_label(
            "把图标拖到列表里",
            NSFont::boldSystemFontOfSize(13.0),
            NSColor::labelColor(),
            NSRect::new(NSPoint::new(98.0, 66.0), NSSize::new(240.0, 20.0)),
        );
        let hint = make_label(
            "系统设置 › 隐私与安全性 › 完全磁盘访问",
            NSFont::systemFontOfSize(12.0),
            NSColor::secondaryLabelColor(),
            NSRect::new(NSPoint::new(98.0, 23.0), NSSize::new(240.0, 38.0)),
        );

        container.addSubview(&icon_view);
        container.addSubview(&title);
        container.addSubview(&hint);
        window.setContentView(Some(&container));

        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let visible = screen.visibleFrame();
            let frame = window.frame();
            window.setFrameOrigin(NSPoint::new(
                visible.origin.x + visible.size.width - frame.size.width - 24.0,
                visible.origin.y + visible.size.height - frame.size.height - 24.0,
            ));
        }

        window.makeKeyAndOrderFront(None);
        PANEL.with(|slot| *slot.borrow_mut() = Some(window));
        crate::log_info!("已打开完全磁盘访问拖动面板");
        Ok(())
    }

    pub fn close_drag_panel() -> Result<(), String> {
        MainThreadMarker::new().ok_or("拖动面板只能在主线程关闭")?;
        let window = PANEL.with(|slot| slot.borrow_mut().take());
        DRAG_URL.with(|slot| *slot.borrow_mut() = None);
        if let Some(window) = window {
            window.close();
            crate::log_info!("已关闭完全磁盘访问拖动面板");
        }
        Ok(())
    }

    pub fn reveal(bundle: &Path) -> Result<(), String> {
        MainThreadMarker::new().ok_or("访达只能在主线程打开")?;
        let path = NSString::from_str(&bundle.to_string_lossy());
        let url = NSURL::fileURLWithPath(&path);
        let urls = NSArray::from_retained_slice(&[url]);
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
        Ok(())
    }
}
