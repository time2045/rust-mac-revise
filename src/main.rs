#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;
mod platform;
mod service;
mod store;

use model::{AppResult, OperationResult, Settings, Snapshot, Store};
use service::Service;
use std::sync::Arc;
use tauri::State;

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
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            random_mac,
            save_profile,
            delete_profile,
            import_profiles,
            export_profiles,
            save_settings,
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
