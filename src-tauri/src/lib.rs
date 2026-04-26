pub mod app_server;
mod bot_settings;
mod codex_provider;

use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow, WindowEvent};

const TRAY_ID: &str = "codex-manager-tray";
const PROVIDER_MENU_PREFIX: &str = "codex-provider:";
const CODEX_COMMAND: &str = "codex";

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, "open-main", "打开主界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let codex_menu = codex_provider_submenu(app)?;
    Menu::with_items(app, &[&open, &separator, &codex_menu, &separator, &quit])
}

fn codex_provider_submenu(app: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let providers = match codex_provider::default_store_path()
        .and_then(|path| codex_provider::list_provider_views(&path))
    {
        Ok(providers) => providers,
        Err(err) => {
            eprintln!("读取 Codex 供应商失败：{err}");
            Vec::new()
        }
    };

    if providers.is_empty() {
        let empty = MenuItem::with_id(
            app,
            "codex-provider-empty",
            "暂无供应商",
            false,
            None::<&str>,
        )?;
        return Submenu::with_id_and_items(
            app,
            "codex-providers",
            "Codex · 未配置",
            true,
            &[&empty],
        );
    }

    let title = providers
        .iter()
        .find(|provider| provider.active)
        .map(|provider| format!("Codex · {}", provider.name))
        .unwrap_or_else(|| "Codex · default".to_string());

    let items = providers
        .iter()
        .map(|provider| {
            CheckMenuItem::with_id(
                app,
                format!("{PROVIDER_MENU_PREFIX}{}", provider.id),
                &provider.name,
                true,
                provider.active,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let item_refs = items
        .iter()
        .map(|item| item as &dyn IsMenuItem<_>)
        .collect::<Vec<_>>();
    Submenu::with_id_and_items(app, "codex-providers", title, true, &item_refs)
}

fn refresh_tray_menu(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match tray_menu(app) {
        Ok(menu) => {
            if let Err(err) = tray.set_menu(Some(menu)) {
                eprintln!("更新菜单栏菜单失败：{err}");
            }
        }
        Err(err) => eprintln!("构建菜单栏菜单失败：{err}"),
    }
}

fn activate_provider_from_tray(app: &AppHandle, id: &str) {
    match codex_provider::activate_provider(
        &match codex_provider::default_store_path() {
            Ok(path) => path,
            Err(err) => {
                eprintln!("定位供应商配置失败：{err}");
                return;
            }
        },
        &match codex_provider::default_codex_dir() {
            Ok(path) => path,
            Err(err) => {
                eprintln!("定位 Codex 配置目录失败：{err}");
                return;
            }
        },
        id,
    ) {
        Ok(_) => refresh_tray_menu(app),
        Err(err) => eprintln!("切换 Codex 供应商失败：{err}"),
    }
}

#[tauri::command]
async fn list_app_server_threads(
    limit: Option<usize>,
) -> Result<Vec<app_server::AppServerThread>, String> {
    app_server::list_threads(CODEX_COMMAND, limit.unwrap_or(25)).await
}

#[tauri::command]
async fn read_app_server_thread(
    thread_id: String,
) -> Result<app_server::AppServerThreadRead, String> {
    app_server::read_thread(CODEX_COMMAND, &thread_id, true).await
}

#[tauri::command]
async fn list_app_server_thread_turns(
    thread_id: String,
    cursor: Option<String>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    app_server::list_thread_turns_compatible(
        CODEX_COMMAND,
        &thread_id,
        cursor.as_deref(),
        limit.unwrap_or(20),
    )
    .await
}

#[tauri::command]
async fn archive_app_server_thread(thread_id: String) -> Result<(), String> {
    app_server::archive_thread(CODEX_COMMAND, &thread_id).await
}

#[tauri::command]
fn list_codex_providers() -> Result<Vec<codex_provider::CodexProviderView>, String> {
    codex_provider::list_provider_views(&codex_provider::default_store_path()?)
}

#[tauri::command]
fn save_codex_provider(
    app: AppHandle,
    provider: codex_provider::CodexProvider,
) -> Result<codex_provider::CodexProviderView, String> {
    let saved = codex_provider::save_provider(&codex_provider::default_store_path()?, provider)?;
    refresh_tray_menu(&app);
    Ok(saved)
}

#[tauri::command]
fn delete_codex_provider(app: AppHandle, id: String) -> Result<(), String> {
    codex_provider::delete_provider(&codex_provider::default_store_path()?, &id)?;
    refresh_tray_menu(&app);
    Ok(())
}

#[tauri::command]
fn activate_codex_provider(
    app: AppHandle,
    id: String,
) -> Result<codex_provider::CodexProviderView, String> {
    let activated = codex_provider::activate_provider(
        &codex_provider::default_store_path()?,
        &codex_provider::default_codex_dir()?,
        &id,
    )?;
    refresh_tray_menu(&app);
    Ok(activated)
}

#[tauri::command]
fn read_codex_live_config() -> Result<String, String> {
    codex_provider::read_live_config(&codex_provider::default_codex_dir()?)
}

#[tauri::command]
fn get_bot_settings() -> Result<bot_settings::BotSettingsView, String> {
    bot_settings::read_settings(&bot_settings::default_env_path()?)
}

#[tauri::command]
fn save_bot_settings(
    settings: bot_settings::BotSettingsInput,
) -> Result<bot_settings::BotSettingsView, String> {
    bot_settings::save_settings(&bot_settings::default_env_path()?, settings)
}

#[tauri::command]
fn get_telegram_bot_status() -> Result<bot_settings::BotServiceStatus, String> {
    Ok(bot_settings::service_status())
}

#[tauri::command]
fn restart_telegram_bot() -> Result<bot_settings::BotServiceStatus, String> {
    bot_settings::restart_service()
}

#[tauri::command]
fn begin_window_drag(window: WebviewWindow) -> Result<WindowDragStart, String> {
    let position = window.outer_position().map_err(|err| err.to_string())?;
    Ok(WindowDragStart {
        window_x: position.x,
        window_y: position.y,
    })
}

#[tauri::command]
fn move_window_to(window: WebviewWindow, x: i32, y: i32) -> Result<(), String> {
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|err| err.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowDragStart {
    window_x: i32,
    window_y: i32,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            list_app_server_threads,
            read_app_server_thread,
            list_app_server_thread_turns,
            archive_app_server_thread,
            list_codex_providers,
            save_codex_provider,
            delete_codex_provider,
            activate_codex_provider,
            read_codex_live_config,
            get_bot_settings,
            save_bot_settings,
            get_telegram_bot_status,
            restart_telegram_bot,
            begin_window_drag,
            move_window_to
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.app_handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_main_window(&app_handle);
                    }
                });
            }

            let menu = tray_menu(app.app_handle())?;
            let icon = Image::from_bytes(include_bytes!("../icons/tray.png"))?;

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open-main" => show_main_window(app),
                    "quit" => app.exit(0),
                    id if id.starts_with(PROVIDER_MENU_PREFIX) => {
                        activate_provider_from_tray(
                            app,
                            id.trim_start_matches(PROVIDER_MENU_PREFIX),
                        );
                    }
                    _ => refresh_tray_menu(app),
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
