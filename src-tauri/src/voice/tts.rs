use anyhow::{anyhow, Result};
use std::process::Command;

pub fn speak(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let status = Command::new("say").arg(text).status()?;
        if status.success() {
            return Ok(());
        }
        return Err(anyhow!("say command failed"));
    }

    #[cfg(target_os = "linux")]
    {
        let status = Command::new("spd-say").arg(text).status()?;
        if status.success() {
            return Ok(());
        }
        return Err(anyhow!("spd-say command failed"));
    }

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $speak = New-Object System.Speech.Synthesis.SpeechSynthesizer; $speak.Speak('{}');",
            text.replace('"', "\\\"")
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()?;
        if status.success() {
            return Ok(());
        }
        return Err(anyhow!("powershell speech command failed"));
    }

    #[allow(unreachable_code)]
    Err(anyhow!("tts not supported on this platform"))
}
