use anyhow::Result;
use serde::{Deserialize, Serialize};

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
        use enigo::{Enigo, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("enigo init failed: {e}"))?;

        Self::set_clipboard(text)?;

        #[cfg(target_os = "macos")]
        {
            use enigo::Key;
            enigo.key(Key::Meta, enigo::Direction::Press)
                .map_err(|e| anyhow::anyhow!("key press failed: {e}"))?;
            enigo.key(Key::Unicode('v'), enigo::Direction::Click)
                .map_err(|e| anyhow::anyhow!("key click failed: {e}"))?;
            enigo.key(Key::Meta, enigo::Direction::Release)
                .map_err(|e| anyhow::anyhow!("key release failed: {e}"))?;
        }

        #[cfg(target_os = "windows")]
        {
            use enigo::Key;
            enigo.key(Key::Control, enigo::Direction::Press)
                .map_err(|e| anyhow::anyhow!("key press failed: {e}"))?;
            enigo.key(Key::Unicode('v'), enigo::Direction::Click)
                .map_err(|e| anyhow::anyhow!("key click failed: {e}"))?;
            enigo.key(Key::Control, enigo::Direction::Release)
                .map_err(|e| anyhow::anyhow!("key release failed: {e}"))?;
        }

        Ok(())
    }

    fn inject_direct(text: &str) -> Result<()> {
        use enigo::{Enigo, Keyboard, Settings};

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("enigo init failed: {e}"))?;

        enigo.text(text)
            .map_err(|e| anyhow::anyhow!("text injection failed: {e}"))?;

        Ok(())
    }

    fn set_clipboard(text: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            let mut child = Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()?;
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
        }

        #[cfg(target_os = "windows")]
        {
            use std::process::Command;
            let mut child = Command::new("clip")
                .stdin(std::process::Stdio::piped())
                .spawn()?;
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
        }

        Ok(())
    }
}
