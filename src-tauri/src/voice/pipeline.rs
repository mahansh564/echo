use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};

use crate::{
    config::EchoConfig,
    db::Db,
    terminal::TerminalManager,
    voice::{asr, audio, intent, router, tts, wake_word},
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
    target_agent_id: Option<i64>,
    target_session_id: Option<i64>,
    text: Option<String>,
    result: String,
    at: String,
}

#[derive(Clone, Serialize)]
struct VoiceErrorEvent {
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceStatusReplyEvent {
    request_type: String,
    target_agent_id: Option<i64>,
    summary: String,
    at: String,
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
        self.handle_transcript(app, db, terminal, config, transcript)
            .await
    }

    pub async fn process_push_to_talk(
        &self,
        app: &AppHandle,
        db: &Db,
        terminal: &TerminalManager,
        config: &EchoConfig,
    ) -> Result<serde_json::Value> {
        let was_running = {
            let inner = self.inner.lock().unwrap();
            inner.running
        };

        self.set_state(app, VoiceRuntimeState::Listening)?;
        let transcript = self.capture_command_transcript(config).await?;
        let result = self
            .handle_transcript(app, db, terminal, config, transcript)
            .await;

        if !was_running {
            self.set_state(app, VoiceRuntimeState::Idle)?;
        }

        result
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

        let execution =
            router::execute_command(app, db, terminal, &config.model_endpoint, &parsed).await;
        match execution {
            Ok(result) => {
                if should_emit_status_reply(&parsed.action, &result) {
                    if let Some(summary) = build_spoken_status_summary(&parsed.action, &result) {
                        if let Err(err) = tts::speak(&summary) {
                            self.emit_error(app, format!("tts failed: {}", err))?;
                        }
                        app.emit(
                            "voice_status_reply",
                            VoiceStatusReplyEvent {
                                request_type: status_request_type(&parsed.action, &result),
                                target_agent_id: target_agent_id_from_result(&result),
                                summary,
                                at: now_timestamp(),
                            },
                        )?;
                    }
                }
                let execution_event = build_voice_action_executed_event(
                    &parsed.action,
                    &parsed.payload,
                    &result,
                    "ok",
                );
                app.emit(
                    "voice_action_executed",
                    execution_event,
                )?;
                self.set_state(app, VoiceRuntimeState::Listening)?;
                Ok(result)
            }
            Err(err) => {
                let execution_event = build_voice_action_executed_event(
                    &parsed.action,
                    &parsed.payload,
                    &serde_json::json!({ "error": err.to_string() }),
                    "error",
                );
                app.emit(
                    "voice_action_executed",
                    execution_event,
                )?;
                self.emit_error(app, err.to_string())?;
                self.set_state(app, VoiceRuntimeState::Listening)?;
                Err(err)
            }
        }
    }

    async fn capture_command_transcript(&self, config: &EchoConfig) -> Result<String> {
        let command_wav = tauri::async_runtime::spawn_blocking({
            let mic_device = config.mic_device.clone();
            let sample_rate = config.audio_sample_rate;
            let duration = config.audio_max_record_ms;
            move || audio::capture_wav_chunk(&mic_device, sample_rate, duration)
        })
        .await
        .map_err(|err| anyhow!("audio capture task failed: {}", err))??;

        let (sample_rate, command_samples) = audio::decode_wav_pcm16_mono(&command_wav)?;
        let trimmed = audio::trim_with_vad(
            &command_samples,
            sample_rate,
            COMMAND_VAD_THRESHOLD,
            COMMAND_SILENCE_STOP_MS,
            COMMAND_MIN_VOICED_MS,
        );

        if trimmed.is_empty() {
            return Err(anyhow!("no speech detected for push-to-talk"));
        }

        let trimmed_wav = audio::encode_wav_pcm16_mono(sample_rate, &trimmed);
        let transcript = asr::transcribe_wav(config, trimmed_wav).await?;
        let text = transcript.trim().to_string();
        if text.is_empty() {
            return Err(anyhow!("empty transcript from push-to-talk"));
        }

        Ok(text)
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

fn build_voice_action_executed_event(
    action: &str,
    payload: &Value,
    result: &Value,
    outcome: &str,
) -> VoiceCommandExecutedEvent {
    VoiceCommandExecutedEvent {
        action: action.to_string(),
        target_agent_id: target_agent_id_from_result(result)
            .or_else(|| payload.get("agent_id").and_then(|value| value.as_i64())),
        target_session_id: target_session_id_from_result(result)
            .or_else(|| payload.get("session_id").and_then(|value| value.as_i64())),
        text: payload
            .get("input")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
        result: outcome.to_string(),
        at: now_timestamp(),
    }
}

fn target_agent_id_from_result(result: &Value) -> Option<i64> {
    result
        .get("targetAgentId")
        .and_then(|value| value.as_i64())
        .or_else(|| result.get("agentId").and_then(|value| value.as_i64()))
        .or_else(|| {
            result
                .get("agent")
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_i64())
        })
}

fn target_session_id_from_result(result: &Value) -> Option<i64> {
    result
        .get("targetSessionId")
        .and_then(|value| value.as_i64())
        .or_else(|| result.get("sessionId").and_then(|value| value.as_i64()))
        .or_else(|| {
            result
                .get("session")
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_i64())
        })
}

fn now_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn should_emit_status_reply(action: &str, result: &Value) -> bool {
    matches!(
        result.get("type").and_then(|value| value.as_str()),
        Some("status_reply" | "status_overview" | "input_needed_list")
    ) || matches!(
        action,
        "status_agent" | "status_overview" | "list_input_needed" | "query_agent_status"
    )
}

fn status_request_type(action: &str, result: &Value) -> String {
    if let Some(result_type) = result.get("type").and_then(|value| value.as_str()) {
        return result_type.to_string();
    }
    action.to_string()
}

fn build_spoken_status_summary(_action: &str, result: &Value) -> Option<String> {
    let result_type = result.get("type").and_then(|value| value.as_str());
    match result_type {
        Some("input_needed_list") => {
            let alerts = result
                .get("alerts")
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            let unresolved = alerts.len();
            if unresolved == 0 {
                return Some("No agents need input right now.".to_string());
            }

            let mut sample_reasons = Vec::new();
            for alert in alerts.iter().take(2) {
                if let Some(reason) = alert.get("reason").and_then(|value| value.as_str()) {
                    let reason_text = reason.replace('_', " ");
                    if !reason_text.trim().is_empty() {
                        sample_reasons.push(reason_text);
                    }
                }
            }

            let reason_clause = if sample_reasons.is_empty() {
                String::new()
            } else {
                format!(" Top reasons: {}.", sample_reasons.join(", "))
            };
            Some(format!(
                "{} unresolved input requests need attention.{}",
                unresolved, reason_clause
            ))
        }
        _ => result
            .get("answer")
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_action_event_uses_result_targets() {
        let payload = serde_json::json!({ "agent_id": 1, "session_id": 2, "input": "run tests" });
        let result = serde_json::json!({ "agentId": 12, "sessionId": 77 });
        let event = build_voice_action_executed_event("send_input", &payload, &result, "ok");
        assert_eq!(event.action, "send_input");
        assert_eq!(event.target_agent_id, Some(12));
        assert_eq!(event.target_session_id, Some(77));
        assert_eq!(event.text.as_deref(), Some("run tests"));
        assert_eq!(event.result, "ok");
    }

    #[test]
    fn voice_action_event_falls_back_to_payload_targets() {
        let payload = serde_json::json!({ "agent_id": 5, "session_id": 6 });
        let result = serde_json::json!({});
        let event = build_voice_action_executed_event("attach_agent", &payload, &result, "ok");
        assert_eq!(event.target_agent_id, Some(5));
        assert_eq!(event.target_session_id, Some(6));
    }

    #[test]
    fn spoken_summary_prioritizes_alert_rollup() {
        let result = serde_json::json!({
            "type": "input_needed_list",
            "alerts": [
                { "reason": "approval_needed" },
                { "reason": "auth_needed" },
                { "reason": "tool_confirmation" }
            ]
        });
        let summary = build_spoken_status_summary("list_input_needed", &result)
            .expect("summary must exist");
        assert!(summary.contains("3 unresolved input requests need attention."));
        assert!(summary.contains("approval needed"));
        assert!(summary.contains("auth needed"));
    }

    #[test]
    fn spoken_summary_uses_answer_for_direct_status_reply() {
        let result = serde_json::json!({
            "type": "status_reply",
            "answer": "Agent Alpha is active."
        });
        let summary =
            build_spoken_status_summary("status_agent", &result).expect("summary must exist");
        assert_eq!(summary, "Agent Alpha is active.");
    }

    #[test]
    fn status_reply_detection_uses_result_type() {
        let result = serde_json::json!({ "type": "status_overview" });
        assert!(should_emit_status_reply("attach_agent", &result));
    }
}
