use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EchoConfig {
    pub mic_device: String,
    pub hotkey: String,
    pub model_endpoint: String,
    pub voice_enabled: bool,
    pub voice_summary_loop_enabled: bool,
    pub voice_summary_loop_interval_sec: u64,
    pub wake_word_model_path: String,
    pub wake_word_phrase: String,
    pub wake_word_sensitivity: f32,
    pub asr_backend: String,
    pub asr_sidecar_path: String,
    pub asr_model_path: String,
    pub asr_endpoint: String,
    pub asr_language: String,
    pub asr_timeout_ms: u64,
    pub audio_sample_rate: u32,
    pub audio_pre_roll_ms: u64,
    pub audio_max_record_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialConfig {
    pub mic_device: Option<String>,
    pub hotkey: Option<String>,
    pub model_endpoint: Option<String>,
    pub voice_enabled: Option<bool>,
    pub voice_summary_loop_enabled: Option<bool>,
    pub voice_summary_loop_interval_sec: Option<u64>,
    pub wake_word_model_path: Option<String>,
    pub wake_word_phrase: Option<String>,
    pub wake_word_sensitivity: Option<f32>,
    pub asr_backend: Option<String>,
    pub asr_sidecar_path: Option<String>,
    pub asr_model_path: Option<String>,
    pub asr_endpoint: Option<String>,
    pub asr_language: Option<String>,
    pub asr_timeout_ms: Option<u64>,
    pub audio_sample_rate: Option<u32>,
    pub audio_pre_roll_ms: Option<u64>,
    pub audio_max_record_ms: Option<u64>,
}

impl Default for EchoConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            mic_device: ":1".to_string(),
            hotkey: "cmd+shift+space".to_string(),
            model_endpoint: "http://localhost:11434".to_string(),
            voice_enabled: true,
            voice_summary_loop_enabled: false,
            voice_summary_loop_interval_sec: 120,
            wake_word_model_path: format!("{}/.echo/wake_words/echo.rpw", home),
            wake_word_phrase: "echo".to_string(),
            wake_word_sensitivity: 0.5,
            asr_backend: "sidecar".to_string(),
            asr_sidecar_path: "whisper-cli".to_string(),
            asr_model_path: format!("{}/.echo/models/ggml-tiny.en.bin", home),
            asr_endpoint: "http://localhost:8080/inference".to_string(),
            asr_language: "en".to_string(),
            asr_timeout_ms: 15_000,
            audio_sample_rate: 16_000,
            audio_pre_roll_ms: 500,
            audio_max_record_ms: 8_000,
        }
    }
}

pub fn load_config() -> Result<EchoConfig> {
    let mut config = EchoConfig::default();
    if let Some(path) = user_config_path() {
        if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            let partial: PartialConfig = toml::from_str(&contents)?;
            if let Some(value) = partial.mic_device {
                config.mic_device = value;
            }
            if let Some(value) = partial.hotkey {
                config.hotkey = value;
            }
            if let Some(value) = partial.model_endpoint {
                config.model_endpoint = value;
            }
            if let Some(value) = partial.voice_enabled {
                config.voice_enabled = value;
            }
            if let Some(value) = partial.voice_summary_loop_enabled {
                config.voice_summary_loop_enabled = value;
            }
            if let Some(value) = partial.voice_summary_loop_interval_sec {
                config.voice_summary_loop_interval_sec = value.max(15);
            }
            if let Some(value) = partial.wake_word_model_path {
                config.wake_word_model_path = value;
            }
            if let Some(value) = partial.wake_word_phrase {
                config.wake_word_phrase = value;
            }
            if let Some(value) = partial.wake_word_sensitivity {
                config.wake_word_sensitivity = value;
            }
            if let Some(value) = partial.asr_backend {
                config.asr_backend = value;
            }
            if let Some(value) = partial.asr_sidecar_path {
                config.asr_sidecar_path = value;
            }
            if let Some(value) = partial.asr_model_path {
                config.asr_model_path = value;
            }
            if let Some(value) = partial.asr_endpoint {
                config.asr_endpoint = value;
            }
            if let Some(value) = partial.asr_language {
                config.asr_language = value;
            }
            if let Some(value) = partial.asr_timeout_ms {
                config.asr_timeout_ms = value;
            }
            if let Some(value) = partial.audio_sample_rate {
                config.audio_sample_rate = value;
            }
            if let Some(value) = partial.audio_pre_roll_ms {
                config.audio_pre_roll_ms = value;
            }
            if let Some(value) = partial.audio_max_record_ms {
                config.audio_max_record_ms = value;
            }
        }
    }
    Ok(config)
}

fn user_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)?;
    Some(home.join(".echo").join("config.toml"))
}

#[cfg(test)]
mod tests;
