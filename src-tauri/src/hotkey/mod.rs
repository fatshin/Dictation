#[cfg(target_os = "macos")]
pub mod fn_key;

#[cfg(target_os = "macos")]
pub use fn_key::start_fn_key_listener;

#[cfg(not(target_os = "macos"))]
pub fn start_fn_key_listener(_app: tauri::AppHandle) {
    log::info!("fn-key long-press listener: macOS only, skipped on this platform");
}
