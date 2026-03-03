use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use anyhow::{anyhow, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri::async_runtime::JoinHandle;

use crate::{
    config::EchoConfig,
    db::Db,
    terminal::TerminalManager,
    voice::{audio, asr, intent, router, tts, wake_word},
};

const WAKE_SCAN_WINDOW_MS: u64 = 1200;
const WAKE_SCAN_MIN_RMS: f32 = 0.01;
const COMMAND_MIN_VOICED_MS: u64 = 350;
const COMMAND_SILENCE_STOP_MS: u64 = 900;
const COMMAND_VAD_THRESHOLD: f32 = 0.015;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceRuntimeState {
    Idle,
    Listening,
    WakeDetected,
    Transcribing,
    Parsing,
    Executing,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    pub running: bool,
    pub state: VoiceRuntimeState,
    pub last_transcript: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceStateUpdatedEvent {
    state: VoiceRuntimeState,
}

#[derive(Clone, Serialize)]
struct VoiceTranscriptEvent {
    text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceIntentEvent {
    action: String,
    payload: serde_json::Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceCommandExecutedEvent {
    action: String,
    success: bool,
    result: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct VoiceErrorEvent {
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceStatusReplyEvent {
    text: String,
    query: String,
    resolved: serde_json::Value,
}

#[derive(Default)]
struct VoiceInner {
    running: bool,
    state: VoiceRuntimeState,
    last_transcript: Option<String>,
    stop_flag: Option<Arc<AtomicBool>>,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone, Default)]
pub struct VoiceManager {
    inner: Arc<Mutex<VoiceInner>>,
}

impl Default for VoiceRuntimeState {
    fn default() -> Self {
        Self::Idle
    }
}

impl VoiceManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> VoiceStatus {
        let inner = self.inner.lock().unwrap();
        VoiceStatus {
            running: inner.running,
            state: inner.state.clone(),
            last_transcript: inner.last_transcript.clone(),
        }
    }

    pub fn start(
        &self,
        app: &AppHandle,
        config: &EchoConfig,
        db: Db,
        terminal: TerminalManager,
    ) -> Result<VoiceStatus> {
        let mut inner = self.inner.lock().unwrap();
        if inner.running {
            return Ok(VoiceStatus {
                running: true,
                state: inner.state.clone(),
                last_transcript: inner.last_transcript.clone(),
            });
        }

        inner.running = true;
        inner.state = VoiceRuntimeState::Listening;
        app.emit(
            "voice_state_updated",
            VoiceStateUpdatedEvent {
                state: inner.state.clone(),
            },
        )?;

        // Wake phrase matching is ASR-based in the current pipeline. Keep model-path
        // validation non-blocking and non-noisy so missing model files do not surface as
        // runtime command errors.
        let _ = wake_word::validate_wake_word_model(&config.wake_word_model_path);

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_for_task = Arc::clone(&stop_flag);
        let app_handle = app.clone();
        let config_clone = config.clone();
        let manager = self.clone();

        let handle = tauri::async_runtime::spawn(async move {
            manager
                .background_loop(app_handle, db, terminal, config_clone, stop_flag_for_task)
                .await;
        });

        inner.stop_flag = Some(stop_flag);
        inner.task = Some(handle);

        Ok(VoiceStatus {
            running: inner.running,
            state: inner.state.clone(),
            last_transcript: inner.last_transcript.clone(),
        })
    }

    pub fn stop(&self, app: &AppHandle) -> Result<VoiceStatus> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(stop_flag) = inner.stop_flag.take() {
            stop_flag.store(true, Ordering::SeqCst);
        }
        if let Some(task) = inner.task.take() {
            task.abort();
        }

        inner.running = false;
        inner.state = VoiceRuntimeState::Idle;
        app.emit(
            "voice_state_updated",
            VoiceStateUpdatedEvent {
                state: inner.state.clone(),
            },
        )?;

        Ok(VoiceStatus {
            running: inner.running,
            state: inner.state.clone(),
            last_transcript: inner.last_transcript.clone(),
        })
    }

    pub async fn process_text(
        &self,
        app: &AppHandle,
        db: &Db,
        terminal: &TerminalManager,
        config: &EchoConfig,
        transcript: String,
    ) -> Result<serde_json::Value> {
        {
            let inner = self.inner.lock().unwrap();
            if !inner.running {
                return Err(anyhow!("voice pipeline is not running"));
            }
        }

        self.set_state(app, VoiceRuntimeState::WakeDetected)?;
        self.set_state(app, VoiceRuntimeState::Transcribing)?;
        self.handle_transcript(app, db, terminal, config, transcript).await
    }

    async fn background_loop(
        &self,
        app: AppHandle,
        db: Db,
        terminal: TerminalManager,
        config: EchoConfig,
        stop_flag: Arc<AtomicBool>,
    ) {
        loop {
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }

            let wake_wav = match tauri::async_runtime::spawn_blocking({
                let mic_device = config.mic_device.clone();
                let sample_rate = config.audio_sample_rate;
                move || audio::capture_wav_chunk(&mic_device, sample_rate, WAKE_SCAN_WINDOW_MS)
            })
            .await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(err)) => {
                    let _ = self.emit_error(&app, err.to_string());
                    continue;
                }
                Err(err) => {
                    let _ = self.emit_error(&app, err.to_string());
                    continue;
                }
            };

            let (_, wake_samples) = match audio::decode_wav_pcm16_mono(&wake_wav) {
                Ok(value) => value,
                Err(err) => {
                    let _ = self.emit_error(&app, err.to_string());
                    continue;
                }
            };

            if audio::rms_pcm16(&wake_samples) < WAKE_SCAN_MIN_RMS {
                continue;
            }

            let wake_text = match asr::transcribe_wav(&config, wake_wav).await {
                Ok(value) => value,
                Err(_) => continue,
            };

            if !wake_word::is_wake_detected(&wake_text, &config.wake_word_phrase) {
                continue;
            }

            if self
                .set_state(&app, VoiceRuntimeState::WakeDetected)
                .is_err()
            {
                break;
            }

            if let Some(inline_command) =
                wake_word::extract_command_after_wake(&wake_text, &config.wake_word_phrase)
            {
                let _ = self
                    .handle_transcript(&app, &db, &terminal, &config, inline_command)
                    .await;
                continue;
            }

            let command_wav = match tauri::async_runtime::spawn_blocking({
                let mic_device = config.mic_device.clone();
                let sample_rate = config.audio_sample_rate;
                let duration = config.audio_max_record_ms;
                move || audio::capture_wav_chunk(&mic_device, sample_rate, duration)
            })
            .await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(err)) => {
                    let _ = self.emit_error(&app, err.to_string());
                    let _ = self.set_state(&app, VoiceRuntimeState::Listening);
                    continue;
                }
                Err(err) => {
                    let _ = self.emit_error(&app, err.to_string());
                    let _ = self.set_state(&app, VoiceRuntimeState::Listening);
                    continue;
                }
            };

            let (sample_rate, command_samples) = match audio::decode_wav_pcm16_mono(&command_wav) {
                Ok(value) => value,
                Err(err) => {
                    let _ = self.emit_error(&app, err.to_string());
                    let _ = self.set_state(&app, VoiceRuntimeState::Listening);
                    continue;
                }
            };

            let trimmed = audio::trim_with_vad(
                &command_samples,
                sample_rate,
                COMMAND_VAD_THRESHOLD,
                COMMAND_SILENCE_STOP_MS,
                COMMAND_MIN_VOICED_MS,
            );
            if trimmed.is_empty() {
                let _ = self.set_state(&app, VoiceRuntimeState::Listening);
                continue;
            }

            let trimmed_wav = audio::encode_wav_pcm16_mono(sample_rate, &trimmed);
            let _ = self.set_state(&app, VoiceRuntimeState::Transcribing);

            let transcript = match asr::transcribe_wav(&config, trimmed_wav).await {
                Ok(text) if !text.trim().is_empty() => text,
                Ok(_) => {
                    let _ = self.set_state(&app, VoiceRuntimeState::Listening);
                    continue;
                }
                Err(err) => {
                    let _ = self.emit_error(&app, err.to_string());
                    let _ = self.set_state(&app, VoiceRuntimeState::Listening);
                    continue;
                }
            };

            let _ = self
                .handle_transcript(&app, &db, &terminal, &config, transcript)
                .await;
        }
    }

    async fn handle_transcript(
        &self,
        app: &AppHandle,
        db: &Db,
        terminal: &TerminalManager,
        config: &EchoConfig,
        transcript: String,
    ) -> Result<serde_json::Value> {
        let text = transcript.trim().to_string();
        if text.is_empty() {
            self.set_state(app, VoiceRuntimeState::Listening)?;
            return Err(anyhow!("empty transcript"));
        }

        {
            let mut inner = self.inner.lock().unwrap();
            inner.last_transcript = Some(text.clone());
        }
        app.emit(
            "voice_transcript",
            VoiceTranscriptEvent { text: text.clone() },
        )?;

        self.set_state(app, VoiceRuntimeState::Parsing)?;

        let parsed = intent::parse_intent(&config.model_endpoint, &text).await?;
        app.emit(
            "voice_intent",
            VoiceIntentEvent {
                action: parsed.action.clone(),
                payload: parsed.payload.clone(),
            },
        )?;

        self.set_state(app, VoiceRuntimeState::Executing)?;

        let execution = router::execute_command(
            app,
            db,
            terminal,
            &config.model_endpoint,
            &parsed,
        )
        .await;
        match execution {
            Ok(result) => {
                if parsed.action == "query_agent_status" {
                    let answer = result
                        .get("answer")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            result
                                .get("statusReply")
                                .and_then(|v| v.get("answer"))
                                .and_then(|v| v.as_str())
                        });
                    if let Some(answer) = answer {
                        if let Err(err) = tts::speak(answer) {
                            self.emit_error(app, format!("tts failed: {}", err))?;
                        }
                        app.emit(
                            "voice_status_reply",
                            VoiceStatusReplyEvent {
                                text: answer.to_string(),
                                query: text.clone(),
                                resolved: result
                                    .get("resolved")
                                    .cloned()
                                    .unwrap_or_else(|| serde_json::json!({})),
                            },
                        )?;
                    }
                }
                app.emit(
                    "voice_command_executed",
                    VoiceCommandExecutedEvent {
                        action: parsed.action,
                        success: true,
                        result: result.clone(),
                    },
                )?;
                self.set_state(app, VoiceRuntimeState::Listening)?;
                Ok(result)
            }
            Err(err) => {
                app.emit(
                    "voice_command_executed",
                    VoiceCommandExecutedEvent {
                        action: parsed.action,
                        success: false,
                        result: serde_json::json!({ "error": err.to_string() }),
                    },
                )?;
                self.emit_error(app, err.to_string())?;
                self.set_state(app, VoiceRuntimeState::Listening)?;
                Err(err)
            }
        }
    }

    fn set_state(&self, app: &AppHandle, state: VoiceRuntimeState) -> Result<()> {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.state = state.clone();
        }
        app.emit("voice_state_updated", VoiceStateUpdatedEvent { state })?;
        Ok(())
    }

    fn emit_error(&self, app: &AppHandle, message: String) -> Result<()> {
        app.emit("voice_error", VoiceErrorEvent { message })?;
        Ok(())
    }
}
