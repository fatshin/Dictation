// Track the frontmost app and emit a `focus:external` event the moment the
// user moves focus from Dictation to anything else. The frontend uses this
// to fire Cmd+V at exactly the right time — no fragile activate-previous-app
// dance. macOS-only.

#[cfg(target_os = "macos")]
pub use mac::{is_external_focused, start_focus_tracker};

#[cfg(not(target_os = "macos"))]
pub fn start_focus_tracker(_app: tauri::AppHandle) {}

#[cfg(not(target_os = "macos"))]
pub fn is_external_focused() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod mac {
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;
    use tauri::{AppHandle, Emitter};

    static EXTERNAL_FOCUSED: OnceLock<AtomicBool> = OnceLock::new();

    fn flag() -> &'static AtomicBool {
        EXTERNAL_FOCUSED.get_or_init(|| AtomicBool::new(false))
    }

    fn current_frontmost() -> Option<String> {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(
                r#"tell application "System Events" to get name of first application process whose frontmost is true"#,
            )
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    fn is_self(name: &str) -> bool {
        // Dev runs as `dictation`, packaged build as `Dictation`.
        name.eq_ignore_ascii_case("dictation")
    }

    pub fn start_focus_tracker(app: AppHandle) {
        std::thread::Builder::new()
            .name("focus-tracker".into())
            .spawn(move || {
                let mut last_was_self: Option<bool> = None;
                loop {
                    if let Some(name) = current_frontmost() {
                        let now_self = is_self(&name);
                        flag().store(!now_self, Ordering::SeqCst);

                        let transitioned_to_external =
                            matches!(last_was_self, Some(true)) && !now_self;
                        if transitioned_to_external {
                            let _ = app.emit("focus:external", &name);
                        }
                        last_was_self = Some(now_self);
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .ok();
    }

    /// Cached read of "is the frontmost app something other than Dictation".
    /// Refreshed by the polling thread every 250ms.
    pub fn is_external_focused() -> bool {
        flag().load(Ordering::SeqCst)
    }
}
