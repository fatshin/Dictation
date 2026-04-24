pub mod asr;
pub mod audio;
pub mod commands;
pub mod db;
pub mod error;
pub mod inject;
pub mod keystore;
pub mod llm;
pub mod session;

use asr::AsrState;
use db::DbState;
use llm::LlmState;
use session::SessionState;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(LlmState::new())
        .manage(AsrState::new())
        .manage(DbState::new())
        .manage(SessionState::new())
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_models,
            commands::rewrite_text,
            commands::rewrite_streaming,
            commands::get_session_info,
            commands::grant_consent,
            commands::start_dictation,
            commands::stop_dictation,
            commands::inject_text,
            commands::search_history,
            commands::list_history,
        ])
        .setup(|app| {
            let show = MenuItemBuilder::with_id("show", "Show Dictation").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
