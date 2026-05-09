pub mod asr;
pub mod audio;
pub mod commands;
pub mod db;
pub mod error;
pub mod hotkey;
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
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

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
            commands::check_setup,
            commands::pull_model,
            commands::get_focused_context,
            commands::build_rewrite_prompt,
        ])
        .setup(|app| {
            // Tray
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

            // Global shortcut: Cmd+Shift+D (Mac) / Ctrl+Shift+D (Win)
            #[cfg(target_os = "macos")]
            let shortcut = Shortcut::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyD);
            #[cfg(not(target_os = "macos"))]
            let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyD);

            let handle = app.handle().clone();
            app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, _event| {
                let _ = handle.emit("hotkey:dictation", ());
            })?;

            // Trigger the Accessibility permission prompt on first run.
            // Without this, AXValue calls return None silently and the user
            // never realises that context-aware correction is broken.
            #[cfg(target_os = "macos")]
            {
                let trusted = inject::ensure_ax_trusted();
                log::info!("AX trusted: {trusted}");
            }

            // fn-key long-press listener (macOS only). Requires Input Monitoring
            // permission; the prompt appears on first run.
            hotkey::start_fn_key_listener(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
