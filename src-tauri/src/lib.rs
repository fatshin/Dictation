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
pub mod vad;

use asr::AsrState;
use db::DbState;
use llm::LlmState;
use session::SessionState;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager, RunEvent, WindowEvent,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            commands::list_audio_devices,
            commands::list_whisper_models,
            commands::download_whisper_model,
            commands::start_dictation,
            commands::stop_dictation,
            commands::inject_text,
            commands::search_history,
            commands::list_history,
            commands::check_setup,
            commands::pull_model,
            commands::get_focused_context,
            commands::build_rewrite_prompt,
            commands::list_dictionary,
            commands::upsert_dictionary_entry,
            commands::delete_dictionary_entry,
            commands::list_prompts,
            commands::upsert_prompt,
            commands::delete_prompt,
            commands::reset_prompt,
            commands::extract_dictionary_block,
            commands::generate_dictionary_candidates,
            commands::set_clipboard_text,
            commands::synth_paste,
            commands::is_external_focused_now,
            commands::get_app_settings,
            commands::update_app_settings,
            commands::get_autostart,
            commands::set_autostart,
        ])
        .setup(|app| {
            // Tray
            let show = MenuItemBuilder::with_id("show", "Show Dictation").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;

            let mut tray = TrayIconBuilder::new();
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            } else {
                log::warn!("default window icon not found; tray icon will use platform default");
            }
            tray.menu(&menu)
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

            // Background tracker that emits `focus:external` whenever the user
            // moves focus from Dictation to another app. The frontend uses
            // this to fire the synthesised Cmd+V at exactly the right moment.
            inject::start_focus_tracker(app.handle().clone());

            // macOS keeps the SQLCipher store. Other platforms intentionally
            // run DB-less and persist settings/prompts/dictionary via JSON
            // fallback in command handlers.
            #[cfg(target_os = "macos")]
            {
                use crate::keystore::{Keystore, MacKeystore};
                let result = (|| -> anyhow::Result<()> {
                    let key = MacKeystore.get_or_create_db_key("com.dictation.app")?;
                    let local_dir = app.path().app_local_data_dir()?;
                    std::fs::create_dir_all(&local_dir)?;
                    let db_path = local_dir.join("dictation.db");
                    let opened = db::EncryptedDb::open(&db_path, &key)?;
                    opened.seed_builtin_prompts(db::BUILTIN_PROMPTS)?;
                    let state: tauri::State<DbState> = app.state();
                    let mut guard = state.db.blocking_lock();
                    *guard = Some(opened);
                    Ok(())
                })();
                if let Err(e) = result {
                    log::info!("DB unavailable; using JSON fallback store: {e:#}");
                }
            }

            #[cfg(not(target_os = "macos"))]
            log::info!("Using JSON fallback store without SQLCipher DB on this platform");

            let settings = {
                let state: tauri::State<DbState> = app.state();
                let guard = state.db.blocking_lock();
                guard
                    .as_ref()
                    .and_then(|db| db.get_app_settings().ok())
                    .or_else(|| commands::load_fallback_app_settings(app.handle()).ok())
                    .unwrap_or_default()
            };
            if let Err(e) =
                hotkey::register_dictation_hotkey(app.handle(), &settings.dictation_hotkey)
            {
                log::warn!("configured dictation hotkey failed; trying default hotkey: {e}");
                hotkey::register_dictation_hotkey(
                    app.handle(),
                    hotkey::DEFAULT_DICTATION_HOTKEY_ID,
                )?;
            }

            Ok(())
        })
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(e) => {
            eprintln!("[dictation] failed to build tauri application: {e}");
            return;
        }
    };

    app.run(|app_handle, event| {
        if let RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } = &event
        {
            if label == "main" {
                // Hide instead of quit so the app stays in the tray.
                // The user can restore via the tray "Show Dictation" item
                // or quit explicitly via "Quit".
                api.prevent_close();
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
        }
    });
}
