// fn-key long-press detector (macOS only).
//
// Listens for the secondary-fn modifier via a CGEventTap in ListenOnly mode so
// single presses keep their default OS behavior (emoji palette, language switch,
// Apple Intelligence). When fn is held for >= LONG_PRESS_MS the listener emits
// `hotkey:press_start`; releasing fn after that emits `hotkey:press_end`.
// Short presses are silently ignored.
//
// Requires macOS Input Monitoring permission (System Settings → Privacy &
// Security → Input Monitoring). The permission prompt is triggered the first
// time CGEventTap is created.

use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const LONG_PRESS_MS: u64 = 500;

struct FnState {
    pressed: AtomicBool,
    active: AtomicBool, // long-press threshold crossed
    press_id: AtomicU64,
}

impl FnState {
    fn new() -> Self {
        Self {
            pressed: AtomicBool::new(false),
            active: AtomicBool::new(false),
            press_id: AtomicU64::new(0),
        }
    }
}

pub fn start_fn_key_listener(app: AppHandle) {
    let spawn_result = std::thread::Builder::new()
        .name("fn-key-tap".into())
        .spawn(move || {
            if let Err(e) = run_event_tap(app) {
                log::error!("fn-key listener exited: {e}");
            }
        });
    if let Err(e) = spawn_result {
        log::error!("failed to spawn fn-key listener thread: {e}");
    }
}

fn run_event_tap(app: AppHandle) -> Result<(), String> {
    let state = Arc::new(FnState::new());

    // Closure captured by CGEventTap. Must not block — schedule timers on a
    // separate thread.
    let cb_state = Arc::clone(&state);
    let cb_app = app.clone();
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged],
        move |_proxy, _ev_type, event| {
            let flags = event.get_flags();
            let now_pressed = flags.contains(CGEventFlags::CGEventFlagSecondaryFn);
            let was_pressed = cb_state.pressed.swap(now_pressed, Ordering::SeqCst);

            if now_pressed && !was_pressed {
                // press: start a delayed activation watcher
                let press_id = cb_state.press_id.fetch_add(1, Ordering::SeqCst) + 1;
                cb_state.active.store(false, Ordering::SeqCst);
                schedule_activation(Arc::clone(&cb_state), cb_app.clone(), press_id);
            } else if !now_pressed && was_pressed {
                // release
                cb_state.press_id.fetch_add(1, Ordering::SeqCst);
                if cb_state.active.swap(false, Ordering::SeqCst) {
                    let _ = cb_app.emit("hotkey:press_end", ());
                }
            }
            None
        },
    )
    .map_err(|_| "CGEventTap::new failed — Input Monitoring permission required".to_string())?;

    let loop_source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|_| "create_runloop_source failed".to_string())?;

    let current = CFRunLoop::get_current();
    unsafe {
        current.add_source(&loop_source, kCFRunLoopCommonModes);
    }
    tap.enable();

    log::info!("fn-key listener: CGEventTap installed, entering run loop");
    CFRunLoop::run_current();
    Ok(())
}

fn schedule_activation(state: Arc<FnState>, app: AppHandle, press_id: u64) {
    std::thread::spawn(move || {
        let start = Instant::now();
        std::thread::sleep(Duration::from_millis(LONG_PRESS_MS));

        // If a release or another press happened, press_id would have advanced.
        if state.press_id.load(Ordering::SeqCst) != press_id {
            return;
        }
        if !state.pressed.load(Ordering::SeqCst) {
            return;
        }
        if state.active.swap(true, Ordering::SeqCst) {
            return; // already active (shouldn't happen)
        }
        log::debug!(
            "fn long-press activated after {}ms",
            start.elapsed().as_millis()
        );
        let _ = app.emit("hotkey:press_start", ());
    });
}
