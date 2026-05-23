use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod context;
pub use context::{
    ensure_ax_trusted, get_focused_field_context, is_ax_trusted, FocusedFieldContext,
};

pub mod focus;
pub use focus::{is_external_focused, start_focus_tracker};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectMode {
    Direct,
    Clipboard,
}

pub struct TextInjector;

impl TextInjector {
    pub fn inject(text: &str, mode: InjectMode) -> Result<()> {
        match mode {
            InjectMode::Direct => Self::inject_direct(text),
            InjectMode::Clipboard => Self::inject_clipboard(text),
        }
    }

    fn inject_clipboard(text: &str) -> Result<()> {
        Self::set_clipboard(text)?;
        Self::synth_paste()
    }

    /// Synthesise the platform paste shortcut (Cmd+V on macOS, Ctrl+V on
    /// Windows). Must be called on the main thread on macOS — enigo's
    /// HIToolbox path asserts main-thread.
    pub fn synth_paste() -> Result<()> {
        use enigo::{Enigo, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("enigo init failed: {e}"))?;

        #[cfg(target_os = "macos")]
        {
            use enigo::Key;
            enigo
                .key(Key::Meta, enigo::Direction::Press)
                .map_err(|e| anyhow::anyhow!("key press failed: {e}"))?;
            enigo
                .key(Key::Unicode('v'), enigo::Direction::Click)
                .map_err(|e| anyhow::anyhow!("key click failed: {e}"))?;
            enigo
                .key(Key::Meta, enigo::Direction::Release)
                .map_err(|e| anyhow::anyhow!("key release failed: {e}"))?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::Key;
            enigo
                .key(Key::Control, enigo::Direction::Press)
                .map_err(|e| anyhow::anyhow!("key press failed: {e}"))?;
            enigo
                .key(Key::Unicode('v'), enigo::Direction::Click)
                .map_err(|e| anyhow::anyhow!("key click failed: {e}"))?;
            enigo
                .key(Key::Control, enigo::Direction::Release)
                .map_err(|e| anyhow::anyhow!("key release failed: {e}"))?;
        }

        Ok(())
    }

    fn inject_direct(text: &str) -> Result<()> {
        use enigo::{Enigo, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("enigo init failed: {e}"))?;

        enigo
            .text(text)
            .map_err(|e| anyhow::anyhow!("text injection failed: {e}"))?;

        Ok(())
    }

    pub fn set_clipboard(text: &str) -> Result<()> {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| anyhow::anyhow!("clipboard init failed: {e}"))?;
        clipboard
            .set_text(text)
            .map_err(|e| anyhow::anyhow!("clipboard set_text failed: {e}"))?;
        Ok(())
    }
}
