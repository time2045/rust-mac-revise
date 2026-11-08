use crate::{model::AppResult, platform};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, MenuBuilder, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Wry,
};

static TRAY_READY: AtomicBool = AtomicBool::new(false);
static EXITING: AtomicBool = AtomicBool::new(false);
static AUTOSTART_ITEM: Mutex<Option<CheckMenuItem<Wry>>> = Mutex::new(None);

pub fn is_ready() -> bool {
    TRAY_READY.load(Ordering::SeqCst)
}

pub fn is_exiting() -> bool {
    EXITING.load(Ordering::SeqCst)
}

pub fn mark_exiting() {
    EXITING.store(true, Ordering::SeqCst);
}

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn sync_autostart(enabled: bool) {
    if let Ok(item) = AUTOSTART_ITEM.lock() {
        if let Some(item) = item.as_ref() {
            let _ = item.set_checked(enabled);
        }
    }
}

fn tray_error(error: impl std::fmt::Display) -> String {
    format!("无法创建系统托盘：{error}")
}

pub fn setup(app: &AppHandle) -> AppResult<()> {
    let show =
        MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>).map_err(tray_error)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "开机自启",
        true,
        platform::is_autostart_enabled(),
        None::<&str>,
    )
    .map_err(tray_error)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>).map_err(tray_error)?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .separator()
        .item(&autostart)
        .separator()
        .item(&quit)
        .build()
        .map_err(tray_error)?;
    let icon = Image::from_bytes(include_bytes!("../icons/tray.png"))
        .map_err(|error| format!("无法加载托盘图标：{error}"))?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("MAC 地址管理器")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "autostart" => {
                let enabled = !platform::is_autostart_enabled();
                match platform::set_autostart(enabled) {
                    Ok(actual) => sync_autostart(actual),
                    Err(error) => platform::show_error(&error),
                }
            }
            "quit" => {
                mark_exiting();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick { .. } => show_main(tray.app_handle()),
            _ => {}
        })
        .build(app)
        .map_err(tray_error)?;

    if let Ok(mut item) = AUTOSTART_ITEM.lock() {
        *item = Some(autostart);
    }
    TRAY_READY.store(true, Ordering::SeqCst);
    Ok(())
}
