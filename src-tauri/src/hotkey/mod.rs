use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg(target_os = "macos")]
pub mod fn_key;

#[cfg(target_os = "macos")]
pub use fn_key::start_fn_key_listener;

#[cfg(not(target_os = "macos"))]
pub fn start_fn_key_listener(_app: tauri::AppHandle) {
    log::info!("fn-key long-press listener: macOS only, skipped on this platform");
}

pub const DEFAULT_DICTATION_HOTKEY_ID: &str = "primary_d";

#[derive(Debug, Clone, Copy)]
struct DictationHotkey {
    id: &'static str,
    shortcut: Shortcut,
}

fn primary_modifier() -> Modifiers {
    #[cfg(target_os = "macos")]
    {
        Modifiers::SUPER
    }
    #[cfg(not(target_os = "macos"))]
    {
        Modifiers::CONTROL
    }
}

fn dictation_hotkeys() -> [DictationHotkey; 4] {
    let primary = primary_modifier();
    [
        DictationHotkey {
            id: "primary_d",
            shortcut: Shortcut::new(Some(primary | Modifiers::SHIFT), Code::KeyD),
        },
        DictationHotkey {
            id: "primary_r",
            shortcut: Shortcut::new(Some(primary | Modifiers::SHIFT), Code::KeyR),
        },
        DictationHotkey {
            id: "primary_space",
            shortcut: Shortcut::new(Some(primary | Modifiers::SHIFT), Code::Space),
        },
        DictationHotkey {
            id: "ctrl_alt_space",
            shortcut: Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space),
        },
    ]
}

fn resolve_hotkey(id: &str) -> DictationHotkey {
    dictation_hotkeys()
        .into_iter()
        .find(|h| h.id == id)
        .unwrap_or_else(|| {
            dictation_hotkeys()
                .into_iter()
                .find(|h| h.id == DEFAULT_DICTATION_HOTKEY_ID)
                .expect("default dictation hotkey must exist")
        })
}

pub fn normalize_dictation_hotkey_id(id: &str) -> String {
    resolve_hotkey(id).id.to_string()
}

pub fn register_dictation_hotkey(app: &AppHandle, id: &str) -> Result<String, String> {
    let selected = resolve_hotkey(id);
    let shortcuts = dictation_hotkeys().map(|h| h.shortcut);
    if let Err(e) = app.global_shortcut().unregister_multiple(shortcuts) {
        log::debug!("dictation hotkey unregister skipped/failed: {e}");
    }

    let handle = app.clone();
    app.global_shortcut()
        .on_shortcut(selected.shortcut, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let _ = handle.emit("hotkey:dictation", ());
            }
        })
        .map_err(|e| format!("register dictation hotkey '{}': {e}", selected.id))?;

    log::info!("dictation hotkey registered: {}", selected.id);
    Ok(selected.id.to_string())
}

#[cfg(test)]
mod tests {
    use super::{normalize_dictation_hotkey_id, DEFAULT_DICTATION_HOTKEY_ID};

    #[test]
    fn normalize_dictation_hotkey_accepts_known_preset() {
        assert_eq!(normalize_dictation_hotkey_id("primary_r"), "primary_r");
    }

    #[test]
    fn normalize_dictation_hotkey_falls_back_to_default() {
        assert_eq!(
            normalize_dictation_hotkey_id("unknown"),
            DEFAULT_DICTATION_HOTKEY_ID
        );
    }
}
