#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;
mod platform;
mod service;
mod store;
mod tray;

use model::{AppResult, OperationResult, Settings, Snapshot, Store};
use service::Service;
use std::sync::Arc;
use tauri::{Manager, State};

type AppState = Arc<Service<platform::WindowsBackend>>;

#[derive(serde::Serialize)]
struct AppInfo {
    version: &'static str,
    repository: &'static str,
    releases: &'static str,
}

async fn blocking<T: Send + 'static>(
    state: State<'_, AppState>,
    operation: impl FnOnce(&Service<platform::WindowsBackend>) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let service = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || operation(&service))
        .await
        .map_err(|e| format!("后台任务失败：{e}"))?
}

#[tauri::command]
async fn get_snapshot(state: State<'_, AppState>) -> AppResult<Snapshot> {
    blocking(state, Service::snapshot).await
}

#[tauri::command]
fn random_mac() -> String {
    model::random_mac()
}

#[tauri::command]
async fn save_profile(
    state: State<'_, AppState>,
    name: String,
    mac: String,
    adapter_guid: Option<String>,
) -> AppResult<Store> {
    blocking(state, move |s| {
        s.save_profile(&name, &mac, adapter_guid.as_deref())
    })
    .await
}

#[tauri::command]
async fn delete_profile(state: State<'_, AppState>, id: String) -> AppResult<Store> {
    blocking(state, move |s| s.delete_profile(&id)).await
}

#[tauri::command]
async fn import_profiles(state: State<'_, AppState>, content: String) -> AppResult<Store> {
    blocking(state, move |s| s.import_profiles(&content)).await
}

#[tauri::command]
async fn export_profiles(state: State<'_, AppState>) -> AppResult<String> {
    blocking(state, Service::export_profiles).await
}

#[tauri::command]
async fn save_settings(state: State<'_, AppState>, settings: Settings) -> AppResult<Store> {
    blocking(state, move |s| s.save_settings(settings)).await
}

#[tauri::command]
async fn set_auto_repair(
    state: State<'_, AppState>,
    guid: String,
    enabled: bool,
) -> AppResult<Store> {
    blocking(state, move |s| s.set_auto_repair(&guid, enabled)).await
}

#[tauri::command]
async fn clear_history(state: State<'_, AppState>) -> AppResult<Store> {
    blocking(state, Service::clear_history).await
}

#[tauri::command]
async fn set_autostart(state: State<'_, AppState>, enabled: bool) -> AppResult<bool> {
    let actual = blocking(state, move |s| s.set_autostart(enabled)).await?;
    tray::sync_autostart(actual);
    Ok(actual)
}

#[tauri::command]
async fn wifi_randomization(guid: String) -> AppResult<platform::WifiReport> {
    tauri::async_runtime::spawn_blocking(move || platform::wifi_randomization(&guid))
        .await
        .map_err(|error| format!("后台任务失败：{error}"))?
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        repository: "https://github.com/time2045/rust-mac-revise",
        releases: "https://github.com/time2045/rust-mac-revise/releases/latest",
    }
}

#[tauri::command]
fn open_project_page(page: String) -> AppResult<()> {
    let url = match page.as_str() {
        "repo" => "https://github.com/time2045/rust-mac-revise",
        "releases" => "https://github.com/time2045/rust-mac-revise/releases/latest",
        _ => return Err("不支持的链接".into()),
    };
    platform::open_url(url)
}

#[tauri::command]
async fn apply_mac(
    state: State<'_, AppState>,
    guid: String,
    mac: String,
) -> AppResult<OperationResult> {
    blocking(state, move |s| s.apply(&guid, Some(&mac))).await
}

#[tauri::command]
async fn auto_repair_mac(
    state: State<'_, AppState>,
    guid: String,
    mac: String,
) -> AppResult<OperationResult> {
    blocking(state, move |s| s.auto_repair(&guid, &mac)).await
}

#[tauri::command]
async fn restore_mac(state: State<'_, AppState>, guid: String) -> AppResult<OperationResult> {
    blocking(state, move |s| s.apply(&guid, None)).await
}

#[tauri::command]
async fn recover_operation(state: State<'_, AppState>) -> AppResult<OperationResult> {
    blocking(state, Service::recover).await
}

#[tauri::command]
fn relaunch_admin(app: tauri::AppHandle) -> AppResult<()> {
    if platform::is_admin() {
        return Ok(());
    }
    platform::relaunch_admin()?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn open_wifi_settings() -> AppResult<()> {
    platform::open_wifi_settings()
}

#[tauri::command]
async fn open_data_folder(state: State<'_, AppState>) -> AppResult<()> {
    blocking(state, |s| platform::open_path(&s.data_dir()?)).await
}

fn main() {
    if let Err(error) = run() {
        if std::env::args().any(|arg| arg == "--diagnose") {
            eprintln!("{error}");
            std::process::exit(1);
        }
        platform::show_error(&error);
    }
}

fn run() -> AppResult<()> {
    let dir = store::data_dir()?;
    if std::env::args().any(|arg| arg == "--diagnose") {
        let service = Service::new(platform::WindowsBackend, store::Persistence::load(dir));
        let result =
            serde_json::to_string_pretty(&service.snapshot()?).map_err(|e| e.to_string())?;
        println!("{result}");
        return Ok(());
    }
    let _instance = platform::lock_instance(&dir)?;
    let state = Arc::new(Service::new(
        platform::WindowsBackend,
        store::Persistence::load(dir),
    ));
    tauri::Builder::default()
        .setup(|app| {
            if let Err(error) = tray::setup(app.handle()) {
                platform::show_error(&error);
            }
            if std::env::args().any(|arg| arg == "--tray") && tray::is_ready() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if tray::is_ready() && !tray::is_exiting() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            set_autostart,
            wifi_randomization,
            get_snapshot,
            random_mac,
            save_profile,
            delete_profile,
            import_profiles,
            export_profiles,
            save_settings,
            clear_history,
            set_auto_repair,
            app_info,
            open_project_page,
            apply_mac,
            auto_repair_mac,
            restore_mac,
            recover_operation,
            relaunch_admin,
            open_wifi_settings,
            open_data_folder,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| format!("无法启动桌面窗口：{e}"))
}
