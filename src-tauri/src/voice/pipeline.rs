use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    config::EchoConfig,
    db::Db,
    telemetry::Telemetry,
    terminal::TerminalManager,
    voice::{asr, audio, intent, router, tts, wake_word},
};

const WAKE_SCAN_WINDOW_MS: u64 = 1200;
const WAKE_SCAN_MIN_RMS: f32 = 0.01;
const COMMAND_MIN_VOICED_MS: u64 = 350;
const COMMAND_SILENCE_STOP_MS: u64 = 900;
const COMMAND_VAD_THRESHOLD: f32 = 0.015;
const VOICE_SUMMARY_MAX_CHARS: usize = 220;
const VOICE_SUMMARY_LLM_THRESHOLD_CHARS: usize = 280;

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

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
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
        let summary_interval = Duration::from_secs(config.voice_summary_loop_interval_sec.max(15));
        let mut next_summary_due = Instant::now() + summary_interval;

        loop {
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }

            if config.voice_summary_loop_enabled && Instant::now() >= next_summary_due {
                if let Err(err) = self.emit_input_needed_summary_loop(&app, &db).await {
                    let _ = self.emit_error(&app, format!("voice summary loop failed: {}", err));
                }
                next_summary_due = Instant::now() + summary_interval;
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

    async fn emit_input_needed_summary_loop(&self, app: &AppHandle, db: &Db) -> Result<()> {
        let alerts = db.list_unresolved_session_alerts(None, Some(200)).await?;
        let agent_count = alerts
            .iter()
            .filter_map(|alert| alert.agent_id)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let alert_count = alerts.len();

        let Some(summary) = build_input_needed_loop_summary(agent_count, alert_count) else {
            return Ok(());
        };

        if let Err(err) = tts::speak(&summary) {
            self.emit_error(app, format!("tts failed: {}", err))?;
        }
        app.emit(
            "voice_status_reply",
            VoiceStatusReplyEvent {
                request_type: "input_needed_summary_loop".to_string(),
                target_agent_id: None,
                summary,
                at: now_timestamp(),
            },
        )?;
        Ok(())
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
                    if let Some(raw_summary) = build_spoken_status_summary(&parsed.action, &result)
                    {
                        let summary =
                            build_voice_summary_for_speech(&config.model_endpoint, &raw_summary)
                                .await;
                        if !summary.is_empty() {
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
                }
                let execution_event = build_voice_action_executed_event(
                    &parsed.action,
                    &parsed.payload,
                    &result,
                    "ok",
                );
                app.emit("voice_action_executed", execution_event)?;
                record_voice_command_metric(app, &parsed.action, "ok", &parsed.payload, &result);
                self.set_state(app, VoiceRuntimeState::Listening)?;
                Ok(result)
            }
            Err(err) => {
                let err_text = err.to_string();
                let error_result = serde_json::json!({ "error": err.to_string() });
                let execution_event = build_voice_action_executed_event(
                    &parsed.action,
                    &parsed.payload,
                    &error_result,
                    "error",
                );
                app.emit("voice_action_executed", execution_event)?;
                record_voice_command_metric(
                    app,
                    &parsed.action,
                    "error",
                    &parsed.payload,
                    &error_result,
                );
                let spoken_error =
                    build_voice_summary_for_speech(&config.model_endpoint, &err_text).await;
                if !spoken_error.is_empty() {
                    if let Err(tts_err) = tts::speak(&spoken_error) {
                        self.emit_error(app, format!("tts failed: {}", tts_err))?;
                    }
                    app.emit(
                        "voice_status_reply",
                        VoiceStatusReplyEvent {
                            request_type: "error".to_string(),
                            target_agent_id: target_agent_id_from_result(&error_result),
                            summary: spoken_error,
                            at: now_timestamp(),
                        },
                    )?;
                }
                self.emit_error(
                    app,
                    sanitize_display_text(&err_text, VOICE_SUMMARY_MAX_CHARS),
                )?;
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
            .map(|value| sanitize_display_text(value, 120)),
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

fn record_voice_command_metric(
    app: &AppHandle,
    action: &str,
    outcome: &str,
    payload: &Value,
    result: &Value,
) {
    let telemetry = app.state::<Telemetry>();
    telemetry.record_voice_command(
        action,
        outcome,
        target_agent_id_from_result(result)
            .or_else(|| payload.get("agent_id").and_then(|value| value.as_i64())),
        target_session_id_from_result(result)
            .or_else(|| payload.get("session_id").and_then(|value| value.as_i64())),
    );
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

fn build_input_needed_loop_summary(agent_count: usize, alert_count: usize) -> Option<String> {
    if agent_count == 0 {
        return None;
    }

    let noun = if agent_count == 1 { "agent" } else { "agents" };
    let verb = if agent_count == 1 { "needs" } else { "need" };
    let request_noun = if alert_count == 1 {
        "request"
    } else {
        "requests"
    };
    Some(format!(
        "{} {} {} input across {} unresolved input {}.",
        agent_count, noun, verb, alert_count, request_noun
    ))
}

async fn build_voice_summary_for_speech(model_endpoint: &str, raw_text: &str) -> String {
    let cleaned = sanitize_display_text(
        raw_text,
        VOICE_SUMMARY_LLM_THRESHOLD_CHARS.saturating_mul(4),
    );
    if cleaned.is_empty() {
        return String::new();
    }

    if should_use_llm_summary(&cleaned) {
        if let Ok(summary) = summarize_with_llm(model_endpoint, &cleaned).await {
            let normalized = sanitize_display_text(&summary, VOICE_SUMMARY_MAX_CHARS);
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }

    sanitize_display_text(&cleaned, VOICE_SUMMARY_MAX_CHARS)
}

async fn summarize_with_llm(model_endpoint: &str, text: &str) -> Result<String> {
    let endpoint = if model_endpoint.contains("/api/") {
        model_endpoint.to_string()
    } else {
        format!("{}/api/generate", model_endpoint.trim_end_matches('/'))
    };

    let prompt = format!(
        "Summarize this runtime output for voice playback in one short sentence (max 25 words). Focus on user action and omit terminal noise.\nText: {}",
        text
    );

    let response = reqwest::Client::new()
        .post(endpoint)
        .json(&serde_json::json!({
            "model": "llama3.2",
            "stream": false,
            "prompt": prompt,
        }))
        .send()
        .await?
        .error_for_status()?;

    let body: OllamaGenerateResponse = response.json().await?;
    Ok(body.response)
}

fn should_use_llm_summary(text: &str) -> bool {
    let lower = text.to_lowercase();
    let noisy_patterns = [
        "traceback",
        "stack trace",
        "stderr",
        "stdout",
        "exception",
        "failed",
        "error:",
        "input:",
    ];

    text.chars().count() > VOICE_SUMMARY_LLM_THRESHOLD_CHARS
        || noisy_patterns.iter().any(|pattern| lower.contains(pattern))
}

fn sanitize_display_text(raw: &str, max_chars: usize) -> String {
    if raw.trim().is_empty() || max_chars == 0 {
        return String::new();
    }

    let stripped = strip_ansi_sequences(raw);
    let mut normalized = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' {
            normalized.push(' ');
            continue;
        }
        if ch.is_control() {
            continue;
        }
        normalized.push(ch);
    }

    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(collapsed.trim(), max_chars)
}

fn strip_ansi_sequences(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }

        let Some(next) = chars.next() else {
            break;
        };
        match next {
            '[' => {
                for seq_char in chars.by_ref() {
                    if ('@'..='~').contains(&seq_char) {
                        break;
                    }
                }
            }
            ']' => {
                let mut prev = '\0';
                for seq_char in chars.by_ref() {
                    if seq_char == '\u{7}' || (prev == '\u{1b}' && seq_char == '\\') {
                        break;
                    }
                    prev = seq_char;
                }
            }
            'P' | '_' | '^' => {
                let mut prev = '\0';
                for seq_char in chars.by_ref() {
                    if prev == '\u{1b}' && seq_char == '\\' {
                        break;
                    }
                    prev = seq_char;
                }
            }
            _ => {}
        }
    }
    out
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 || input.is_empty() {
        return String::new();
    }
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }

    let head = input
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{}…", head)
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
        let summary =
            build_spoken_status_summary("list_input_needed", &result).expect("summary must exist");
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

    #[test]
    fn input_needed_loop_summary_uses_counts() {
        let summary = build_input_needed_loop_summary(2, 3).expect("summary");
        assert_eq!(
            summary,
            "2 agents need input across 3 unresolved input requests."
        );
    }

    #[test]
    fn input_needed_loop_summary_skips_when_none_need_input() {
        assert!(build_input_needed_loop_summary(0, 0).is_none());
    }

    #[test]
    fn sanitize_display_text_removes_ansi_and_control_chars() {
        let value = sanitize_display_text("\u{1b}[31mfailed\u{1b}[0m\tline\0", 64);
        assert_eq!(value, "failed line");
    }

    #[test]
    fn sanitize_display_text_truncates_long_messages() {
        let value = sanitize_display_text("1234567890abcdef", 8);
        assert_eq!(value, "1234567…");
    }

    #[test]
    fn llm_summary_heuristic_uses_noise_and_length() {
        assert!(should_use_llm_summary(
            "error: terminal session output too verbose"
        ));
        assert!(!should_use_llm_summary("agent is active"));
    }
}
