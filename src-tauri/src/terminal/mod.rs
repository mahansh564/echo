use anyhow::{anyhow, Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

use crate::config::EchoConfig;
use crate::db::{models::ManagedSession, models::StartSessionRequest, AlertEnrichmentInput, Db};
use crate::issue_enrichment::enrich_issue_message;
use crate::providers::{adapter_for, ProviderParseState, ProviderStructuredEvent};
use crate::telemetry::Telemetry;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const STALE_THRESHOLD: Duration = Duration::from_secs(20);
const PARSE_ERROR_ALERT_THROTTLE: Duration = Duration::from_secs(30);
const OPENCODE_IDLE_INPUT_THRESHOLD: Duration = Duration::from_secs(2);
const OPENCODE_IDLE_INPUT_REASON: &str = "idle_no_output";
const OPENCODE_IDLE_INPUT_MESSAGE: &str = "Waiting for your input";
const OUTPUT_MONITOR_INTERVAL: Duration = Duration::from_millis(50);
const TERMINAL_SNIPPET_MAX_CHARS: usize = 220;
const PER_SESSION_OUTPUT_MAX_BYTES: usize = 200_000;
const TARGET_ACTIVE_SESSION_CAPACITY: usize = 10;
const TOTAL_OUTPUT_MEMORY_CAP_BYTES: usize =
    PER_SESSION_OUTPUT_MAX_BYTES * TARGET_ACTIVE_SESSION_CAPACITY;
const TMUX_CAPTURE_LINES: &str = "-2000";
const TMUX_SOCKET_FILE: &str = "echo-runtime.sock";
#[cfg(test)]
static NEXT_TEST_SESSION_TOKEN: AtomicU64 = AtomicU64::new(1);

fn apply_terminal_env(cmd: &mut CommandBuilder) {
    let term = std::env::var("TERM").unwrap_or_default();
    if term.trim().is_empty() || term == "dumb" {
        cmd.env("TERM", "xterm-256color");
    } else {
        cmd.env("TERM", term);
    }

    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    if colorterm.trim().is_empty() {
        cmd.env("COLORTERM", "truecolor");
    } else {
        cmd.env("COLORTERM", colorterm);
    }
    cmd.env("TMUX", "");
    cmd.env("TMUX_TMPDIR", tmux_tmpdir());
}

#[derive(Clone)]
pub struct TerminalManager {
    supervisor: Arc<SessionSupervisor>,
    #[cfg(test)]
    next_legacy_id: Arc<AtomicU64>,
}

#[derive(Clone)]
struct SessionSupervisor {
    sessions: Arc<Mutex<HashMap<i64, SessionHandle>>>,
}

#[derive(Clone)]
struct RuntimeEventContext {
    app: AppHandle,
    db: Db,
    session_id: i64,
    agent_id: Option<i64>,
    provider: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSession {
    pub id: u64,
    pub agent_id: i64,
    pub command: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TerminalChunkEvent {
    session_id: i64,
    cursor: usize,
    chunk: String,
    is_delta: bool,
    at: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentRuntimeUpdatedEvent {
    agent_id: i64,
    active_session_id: Option<i64>,
    status: String,
    attention_state: String,
    last_activity_at: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AgentAttentionUpdatedEvent {
    agent_id: i64,
    attention_state: String,
    unresolved_alert_count: i64,
    last_input_required_at: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SessionAlertCreatedEvent {
    alert_id: i64,
    session_id: i64,
    agent_id: Option<i64>,
    severity: String,
    reason: String,
    message: String,
    message_enriched: Option<String>,
    message_enrichment_status: String,
    message_enriched_at: Option<String>,
    requires_ack: bool,
    created_at: String,
}

struct AttachClientHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

struct SessionHandle {
    last_snippet: Arc<Mutex<String>>,
    output: Arc<Mutex<SessionOutputBuffer>>,
    idle_input_marked: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    attach_active: Arc<AtomicBool>,
    attach_client: Mutex<Option<AttachClientHandle>>,
    tmux_session_name: String,
    tmux_pane_id: String,
    _tmux_window_id: String,
    log_path: PathBuf,
    runtime_context: Option<RuntimeEventContext>,
}

#[derive(Clone)]
struct SessionOutputBuffer {
    base_offset: usize,
    data: String,
    max_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TmuxSessionMetadata {
    tmux_session_name: String,
    tmux_pane_id: String,
    tmux_window_id: String,
    log_path: String,
    #[serde(default)]
    attach_client_session_id: Option<String>,
}

struct TmuxCreateResult {
    pane_id: String,
    window_id: String,
}

impl SessionOutputBuffer {
    fn with_limit(max_bytes: usize) -> Self {
        Self {
            base_offset: 0,
            data: String::new(),
            max_bytes: max_bytes.max(1),
        }
    }

    fn append(&mut self, chunk: &str) -> usize {
        self.data.push_str(chunk);
        self.trim_to_limit();
        self.end_cursor()
    }

    fn snapshot(&self) -> String {
        self.data.clone()
    }

    fn chunk(&self, cursor: usize, max_bytes: usize) -> (String, usize, bool) {
        let start_abs = cursor.max(self.base_offset);
        let end_abs = self.end_cursor();
        if start_abs >= end_abs {
            return (String::new(), end_abs, false);
        }

        let start_idx = start_abs - self.base_offset;
        let requested_end = start_idx
            .saturating_add(max_bytes.max(1))
            .min(self.data.len());
        let mut end_idx = clamp_to_char_boundary(&self.data, requested_end);
        if end_idx <= start_idx {
            end_idx = next_char_boundary(&self.data, start_idx);
        }

        let chunk = self.data[start_idx..end_idx].to_string();
        let next_cursor = self.base_offset + end_idx;
        let has_more = next_cursor < end_abs;
        (chunk, next_cursor, has_more)
    }

    fn end_cursor(&self) -> usize {
        self.base_offset + self.data.len()
    }

    fn trim_to_limit(&mut self) {
        if self.data.len() <= self.max_bytes {
            return;
        }

        let overflow = self.data.len() - self.max_bytes;
        let mut trim_at = overflow.min(self.data.len());
        while trim_at < self.data.len() && !self.data.is_char_boundary(trim_at) {
            trim_at += 1;
        }
        if trim_at == 0 {
            return;
        }
        self.data.drain(..trim_at);
        self.base_offset += trim_at;
    }
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            supervisor: Arc::new(SessionSupervisor {
                sessions: Arc::new(Mutex::new(HashMap::new())),
            }),
            #[cfg(test)]
            next_legacy_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub async fn start_session(
        &self,
        app: &AppHandle,
        db: Db,
        request: StartSessionRequest,
    ) -> Result<ManagedSession> {
        self.supervisor.start_session(app, db, request).await
    }

    pub fn stop_session(&self, db: Db, session_id: i64) -> Result<()> {
        self.supervisor.stop_session(db, session_id)
    }

    pub fn has_session(&self, session_id: i64) -> bool {
        self.supervisor.has_session(session_id)
    }

    pub fn last_snippet(&self, session_id: i64) -> Option<String> {
        self.supervisor.last_snippet(session_id)
    }

    pub fn list_runtime_sessions(&self) -> Vec<i64> {
        self.supervisor.list_runtime_sessions()
    }

    pub fn session_output(&self, session_id: i64) -> Option<String> {
        self.supervisor.session_output(session_id)
    }

    pub fn session_output_chunk(
        &self,
        session_id: i64,
        cursor: usize,
        max_bytes: usize,
    ) -> Option<(String, usize, bool)> {
        self.supervisor
            .session_output_chunk(session_id, cursor, max_bytes)
    }

    pub fn send_input(&self, session_id: i64, input: &str) -> Result<()> {
        self.supervisor.send_input(session_id, input)
    }

    pub async fn attach_session(&self, db: &Db, session_id: i64) -> Result<ManagedSession> {
        self.supervisor.attach_session(db, session_id).await
    }

    pub async fn detach_session(&self, db: &Db, session_id: i64) -> Result<ManagedSession> {
        self.supervisor.detach_session(db, session_id).await
    }

    pub fn resize_session(&self, session_id: i64, cols: u16, rows: u16) -> Result<()> {
        self.supervisor.resize_session(session_id, cols, rows)
    }

    pub async fn reconcile_orphan_sessions(&self, db: &Db) -> Result<usize> {
        self.supervisor.reconcile_orphan_sessions(None, db).await
    }

    pub async fn reconcile_orphan_sessions_with_app(
        &self,
        app: &AppHandle,
        db: &Db,
    ) -> Result<usize> {
        self.supervisor.reconcile_orphan_sessions(Some(app), db).await
    }

    pub async fn start_session_legacy(
        &self,
        app: &AppHandle,
        db: Db,
        agent_id: i64,
    ) -> Result<TerminalSession> {
        let session = self
            .start_session(
                app,
                db,
                StartSessionRequest {
                    command: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string()),
                    args: vec!["-l".to_string()],
                    cwd: None,
                    agent_id: Some(agent_id),
                    task_id: None,
                    provider: Some("opencode".to_string()),
                },
            )
            .await?;
        Ok(TerminalSession {
            id: session.id as u64,
            agent_id,
            command: format!(
                "{} -l",
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
            ),
        })
    }

    pub fn stop_session_legacy(&self, db: Db, session_id: u64) -> Result<()> {
        self.stop_session(db, session_id as i64)
    }

    #[cfg(test)]
    pub fn start_session_for_test(&self, command: &str, args: &[&str]) -> Result<TerminalSession> {
        ensure_tmux_available()?;

        let id = self.next_legacy_id.fetch_add(1, Ordering::SeqCst);
        let session_id = id as i64;
        let session_name = format!(
            "echo-test-{}-{}-{session_id}",
            std::process::id(),
            NEXT_TEST_SESSION_TOKEN.fetch_add(1, Ordering::SeqCst)
        );
        let log_path = std::env::temp_dir()
            .join("echo-test-tmux")
            .join(format!("{session_name}.log"));
        let arg_values = args.iter().map(|value| (*value).to_string()).collect::<Vec<_>>();
        let tmux = tmux_new_session(&session_name, None, command, &arg_values, &log_path)?;
        let handle = SessionHandle {
            last_snippet: Arc::new(Mutex::new(String::new())),
            output: Arc::new(Mutex::new(SessionOutputBuffer::with_limit(
                PER_SESSION_OUTPUT_MAX_BYTES,
            ))),
            idle_input_marked: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(false)),
            attach_active: Arc::new(AtomicBool::new(false)),
            attach_client: Mutex::new(None),
            tmux_session_name: session_name.clone(),
            tmux_pane_id: tmux.pane_id,
            _tmux_window_id: tmux.window_id,
            log_path,
            runtime_context: None,
        };
        self.supervisor
            .register_session_handle(session_id, handle)
            .context("register test session")?;
        Ok(TerminalSession {
            id,
            agent_id: 0,
            command: format!("{} {}", command, args.join(" ")),
        })
    }
}

impl SessionSupervisor {
    fn register_session_handle(&self, session_id: i64, handle: SessionHandle) -> Result<()> {
        let last_snippet = Arc::clone(&handle.last_snippet);
        let output = Arc::clone(&handle.output);
        let idle_input_marked = Arc::clone(&handle.idle_input_marked);
        let stopped = Arc::clone(&handle.stopped);
        let attach_active = Arc::clone(&handle.attach_active);
        let runtime_context = handle.runtime_context.clone();
        let tmux_session_name = handle.tmux_session_name.clone();
        let tmux_pane_id = handle.tmux_pane_id.clone();
        let log_path = handle.log_path.clone();

        self.sessions.lock().unwrap().insert(session_id, handle);
        spawn_tmux_monitor(
            self.clone(),
            session_id,
            tmux_session_name,
            tmux_pane_id,
            log_path,
            last_snippet,
            output,
            idle_input_marked,
            stopped,
            attach_active,
            runtime_context,
        );
        Ok(())
    }

    async fn reconcile_orphan_sessions(
        &self,
        app: Option<&AppHandle>,
        db: &Db,
    ) -> Result<usize> {
        let rows = db.list_managed_sessions(None, Some(500)).await?;
        let mut reconciled = 0usize;

        for row in rows {
            let is_open = matches!(
                row.status.as_str(),
                "waking" | "active" | "stalled" | "needs_input"
            );
            if !is_open {
                continue;
            }
            if self.has_session(row.id) {
                continue;
            }

            let Some(metadata) = parse_tmux_metadata(row.metadata_json.as_deref()) else {
                db.update_session_status(
                    row.id,
                    "failed",
                    Some("tmux session metadata missing during startup reconciliation"),
                )
                .await?;
                db.insert_session_event(
                    row.id,
                    "orphan_cleanup",
                    Some("session marked failed during startup reconciliation"),
                    Some(
                        &serde_json::json!({
                            "transport": row.transport,
                            "reason": "missing_tmux_metadata"
                        })
                        .to_string(),
                    ),
                )
                .await?;
                reconciled += 1;
                continue;
            };

            if !tmux_has_session(&metadata.tmux_session_name) {
                db.update_session_status(
                    row.id,
                    "failed",
                    Some("tmux session missing during startup reconciliation"),
                )
                .await?;
                db.insert_session_event(
                    row.id,
                    "orphan_cleanup",
                    Some("session marked failed during startup reconciliation"),
                    Some(
                        &serde_json::json!({
                            "transport": "tmux",
                            "tmuxSessionName": metadata.tmux_session_name,
                            "reason": "missing_tmux_session"
                        })
                        .to_string(),
                    ),
                )
                .await?;
                reconciled += 1;
                continue;
            }

            let handle = SessionHandle {
                last_snippet: Arc::new(Mutex::new(String::new())),
                output: Arc::new(Mutex::new(SessionOutputBuffer::with_limit(
                    PER_SESSION_OUTPUT_MAX_BYTES,
                ))),
                idle_input_marked: Arc::new(AtomicBool::new(row.needs_input)),
                stopped: Arc::new(AtomicBool::new(false)),
                attach_active: Arc::new(AtomicBool::new(false)),
                attach_client: Mutex::new(None),
                tmux_session_name: metadata.tmux_session_name.clone(),
                tmux_pane_id: metadata.tmux_pane_id.clone(),
                _tmux_window_id: metadata.tmux_window_id.clone(),
                log_path: PathBuf::from(metadata.log_path),
                runtime_context: app.cloned().map(|app| RuntimeEventContext {
                    app,
                    db: db.clone(),
                    session_id: row.id,
                    agent_id: row.agent_id,
                    provider: row.provider.clone(),
                }),
            };
            self.register_session_handle(row.id, handle)?;
            reconciled += 1;
        }

        Ok(reconciled)
    }

    pub async fn start_session(
        &self,
        app: &AppHandle,
        db: Db,
        request: StartSessionRequest,
    ) -> Result<ManagedSession> {
        let requested_provider = request
            .provider
            .clone()
            .unwrap_or_else(|| "opencode".to_string());
        let adapter = adapter_for(&requested_provider);
        let spawn_spec = match adapter.spawn_session(&request) {
            Ok(spec) => spec,
            Err(err) => {
                record_session_start_failed_metric(
                    app,
                    None,
                    request.agent_id,
                    adapter.provider_name(),
                    "supervisor.spawn_spec",
                    &err.to_string(),
                );
                return Err(err);
            }
        };
        if spawn_spec.command.trim().is_empty() {
            let err = anyhow!("command is required");
            record_session_start_failed_metric(
                app,
                None,
                request.agent_id,
                adapter.provider_name(),
                "supervisor.spawn_spec",
                &err.to_string(),
            );
            return Err(err);
        }

        if let Some(agent_id) = request.agent_id {
            if let Ok(agent) = db.get_agent(agent_id).await {
                if let Some(active_session_id) = agent.active_session_id {
                    let _ = self.stop_session(db.clone(), active_session_id);
                }
            }
        }

        let args_json = serde_json::to_string(&spawn_spec.args)?;
        let managed = match db
            .create_managed_session(
                adapter.provider_name(),
                &spawn_spec.command,
                &args_json,
                spawn_spec.cwd.as_deref(),
                request.agent_id,
                request.task_id,
                None,
            )
            .await
        {
            Ok(managed) => managed,
            Err(err) => {
                record_session_start_failed_metric(
                    app,
                    None,
                    request.agent_id,
                    adapter.provider_name(),
                    "supervisor.create_session_row",
                    &err.to_string(),
                );
                return Err(err);
            }
        };

        if let Err(err) = ensure_tmux_available() {
            let reason = "tmux is not installed or not available on PATH";
            let _ = db.update_session_status(managed.id, "failed", Some(reason)).await;
            let _ = db
                .insert_session_event(
                    managed.id,
                    "error",
                    Some("tmux not installed"),
                    Some(
                        &serde_json::json!({
                            "error": err.to_string(),
                            "transport": "tmux"
                        })
                        .to_string(),
                    ),
                )
                .await;
            emit_runtime_events(app, &db, managed.id).await;
            record_session_start_failed_metric(
                app,
                Some(managed.id),
                request.agent_id,
                adapter.provider_name(),
                "supervisor.ensure_tmux",
                reason,
            );
            return Err(anyhow!(reason));
        }

        let session_name = format!("echo-session-{}", managed.id);
        let log_path = app
            .path()
            .app_data_dir()
            .context("resolve app data dir")?
            .join("tmux-logs")
            .join(format!("{session_name}.log"));

        let tmux_session = match tmux_new_session(
            &session_name,
            spawn_spec.cwd.as_deref(),
            &spawn_spec.command,
            &spawn_spec.args,
            &log_path,
        ) {
            Ok(result) => result,
            Err(err) => {
                let reason = format!("failed to create tmux session: {err}");
                let _ = db
                    .update_session_status(managed.id, "failed", Some(&reason))
                    .await;
                let _ = db
                    .insert_session_event(
                        managed.id,
                        "error",
                        Some("failed to create tmux session"),
                        Some(
                            &serde_json::json!({
                                "error": err.to_string(),
                                "transport": "tmux",
                                "tmuxSessionName": session_name,
                            })
                            .to_string(),
                        ),
                    )
                    .await;
                emit_runtime_events(app, &db, managed.id).await;
                record_session_start_failed_metric(
                    app,
                    Some(managed.id),
                    request.agent_id,
                    adapter.provider_name(),
                    "supervisor.create_tmux_session",
                    &reason,
                );
                return Err(anyhow!(reason));
            }
        };

        let metadata = TmuxSessionMetadata {
            tmux_session_name: session_name.clone(),
            tmux_pane_id: tmux_session.pane_id.clone(),
            tmux_window_id: tmux_session.window_id.clone(),
            log_path: log_path.to_string_lossy().into_owned(),
            attach_client_session_id: None,
        };
        let metadata_json = serde_json::to_string(&metadata)?;
        let _ = db
            .update_session_metadata(managed.id, Some(&metadata_json))
            .await;
        let _ = db.update_session_status(managed.id, "active", None).await;
        let _ = db.update_session_heartbeat(managed.id).await;
        let _ = db
            .insert_session_event(
                managed.id,
                "spawned",
                Some("session started"),
                Some(
                    &serde_json::json!({
                        "command": spawn_spec.command,
                        "args": spawn_spec.args,
                        "cwd": spawn_spec.cwd,
                        "provider": adapter.provider_name(),
                        "supports_attach": adapter.supports_terminal_attach(),
                        "transport": "tmux",
                        "tmuxSessionName": metadata.tmux_session_name,
                        "tmuxPaneId": metadata.tmux_pane_id,
                        "tmuxWindowId": metadata.tmux_window_id,
                    })
                    .to_string(),
                ),
            )
            .await;
        emit_runtime_events(app, &db, managed.id).await;

        debug_assert!(
            TOTAL_OUTPUT_MEMORY_CAP_BYTES
                >= PER_SESSION_OUTPUT_MAX_BYTES * TARGET_ACTIVE_SESSION_CAPACITY
        );
        let handle = SessionHandle {
            last_snippet: Arc::new(Mutex::new(String::new())),
            output: Arc::new(Mutex::new(SessionOutputBuffer::with_limit(
                PER_SESSION_OUTPUT_MAX_BYTES,
            ))),
            idle_input_marked: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(false)),
            attach_active: Arc::new(AtomicBool::new(false)),
            attach_client: Mutex::new(None),
            tmux_session_name: metadata.tmux_session_name.clone(),
            tmux_pane_id: metadata.tmux_pane_id.clone(),
            _tmux_window_id: metadata.tmux_window_id.clone(),
            log_path: PathBuf::from(metadata.log_path.clone()),
            runtime_context: Some(RuntimeEventContext {
                app: app.clone(),
                db: db.clone(),
                session_id: managed.id,
                agent_id: request.agent_id,
                provider: requested_provider.clone(),
            }),
        };
        self.register_session_handle(managed.id, handle)?;

        let latest = db.get_managed_session(managed.id).await?;
        record_session_started_metric(
            app,
            latest.id,
            latest.agent_id,
            &latest.provider,
            "supervisor.start_session",
        );
        Ok(latest)
    }

    pub fn stop_session(&self, db: Db, session_id: i64) -> Result<()> {
        let removed = self.sessions.lock().unwrap().remove(&session_id);
        if let Some(handle) = removed {
            handle.stopped.store(true, Ordering::SeqCst);
            let _ = self.kill_attach_client(&handle);
            let _ = tmux_kill_session(&handle.tmux_session_name);
        }

        tauri::async_runtime::spawn(async move {
            let ended = db
                .end_session_if_open(session_id, Some("stopped by user"))
                .await
                .unwrap_or(false);
            if ended {
                let _ = db
                    .insert_session_event(session_id, "ended", Some("stopped by user"), None)
                    .await;
            }
        });

        Ok(())
    }

    pub async fn attach_session(&self, db: &Db, session_id: i64) -> Result<ManagedSession> {
        if !self.has_session(session_id) {
            return Err(anyhow!("session runtime is not available for attach"));
        }

        self.ensure_attach_client(session_id)?;
        let updated = db.attach_terminal_session(session_id).await?;
        if !self.has_session(session_id) {
            let _ = db.detach_terminal_session(session_id).await;
            return Err(anyhow!("session ended before attach completed"));
        }

        Ok(updated)
    }

    pub async fn detach_session(&self, db: &Db, session_id: i64) -> Result<ManagedSession> {
        let updated = db.detach_terminal_session(session_id).await?;
        if updated.attach_count == 0 {
            self.close_attach_client(session_id)?;
        }
        Ok(updated)
    }

    pub fn last_snippet(&self, session_id: i64) -> Option<String> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions.get(&session_id)?;
        let snippet = handle.last_snippet.lock().unwrap().clone();
        Some(sanitize_terminal_preview(
            &snippet,
            TERMINAL_SNIPPET_MAX_CHARS,
        ))
    }

    pub fn list_runtime_sessions(&self) -> Vec<i64> {
        let sessions = self.sessions.lock().unwrap();
        sessions.keys().copied().collect()
    }

    pub fn has_session(&self, session_id: i64) -> bool {
        let sessions = self.sessions.lock().unwrap();
        sessions.contains_key(&session_id)
    }

    pub fn session_output(&self, session_id: i64) -> Option<String> {
        self.ensure_snapshot_for_session(session_id);
        let sessions = self.sessions.lock().unwrap();
        let output = Arc::clone(&sessions.get(&session_id)?.output);
        drop(sessions);
        let text = output.lock().unwrap().snapshot();
        Some(text)
    }

    pub fn session_output_chunk(
        &self,
        session_id: i64,
        cursor: usize,
        max_bytes: usize,
    ) -> Option<(String, usize, bool)> {
        self.ensure_snapshot_for_session(session_id);
        let sessions = self.sessions.lock().unwrap();
        let output = Arc::clone(&sessions.get(&session_id)?.output);
        drop(sessions);
        let chunk = output.lock().unwrap().chunk(cursor, max_bytes);
        Some(chunk)
    }

    pub fn send_input(&self, session_id: i64, input: &str) -> Result<()> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow!("session not found"))?;
        handle.idle_input_marked.store(false, Ordering::SeqCst);

        if handle.attach_active.load(Ordering::SeqCst) {
            let mut attach = handle.attach_client.lock().unwrap();
            if let Some(client) = attach.as_mut() {
                client.writer.write_all(input.as_bytes())?;
                client.writer.flush()?;
                return Ok(());
            }
        }

        tmux_send_input(&handle.tmux_pane_id, input)
    }

    pub fn resize_session(&self, session_id: i64, cols: u16, rows: u16) -> Result<()> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow!("session not found"))?;
        let mut attach = handle.attach_client.lock().unwrap();
        if let Some(client) = attach.as_mut() {
            client.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
            return Ok(());
        }

        tmux_resize_pane(&handle.tmux_pane_id, cols, rows)
    }

    fn ensure_snapshot_for_session(&self, session_id: i64) {
        let sessions = self.sessions.lock().unwrap();
        let Some(handle) = sessions.get(&session_id) else {
            return;
        };
        if handle.attach_active.load(Ordering::SeqCst) {
            return;
        }
        let should_prime = handle.output.lock().unwrap().end_cursor() == 0;
        if !should_prime {
            return;
        }
        if let Ok(snapshot) = tmux_capture_pane(&handle.tmux_pane_id) {
            if !snapshot.is_empty() {
                let mut output = handle.output.lock().unwrap();
                output.append(&snapshot);
                let preview = sanitize_terminal_preview(&snapshot, TERMINAL_SNIPPET_MAX_CHARS);
                if !preview.is_empty() {
                    *handle.last_snippet.lock().unwrap() = preview;
                }
            }
        }
    }

    fn close_attach_client(&self, session_id: i64) -> Result<()> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow!("session not found"))?;
        self.kill_attach_client(handle)
    }

    fn kill_attach_client(&self, handle: &SessionHandle) -> Result<()> {
        handle.attach_active.store(false, Ordering::SeqCst);
        let mut attach = handle.attach_client.lock().unwrap();
        if let Some(client) = attach.as_mut() {
            let _ = client.child.kill();
        }
        *attach = None;
        Ok(())
    }

    fn ensure_attach_client(&self, session_id: i64) -> Result<()> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow!("session not found"))?;
        if handle.attach_client.lock().unwrap().is_some() {
            return Ok(());
        }

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new("tmux");
        let socket_path = tmux_socket_path();
        let socket_arg = socket_path.to_string_lossy().into_owned();
        cmd.args(&["-S", &socket_arg, "attach-session", "-t", &handle.tmux_session_name]);
        apply_terminal_env(&mut cmd);
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to attach tmux client")?;
        let master = pair.master;
        let mut reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;

        let last_snippet = Arc::clone(&handle.last_snippet);
        let output = Arc::clone(&handle.output);
        let idle_input_marked = Arc::clone(&handle.idle_input_marked);
        let attach_active = Arc::clone(&handle.attach_active);
        let runtime_context = handle.runtime_context.clone();
        let supervisor = self.clone();
        attach_active.store(true, Ordering::SeqCst);
        {
            let mut attach = handle.attach_client.lock().unwrap();
            *attach = Some(AttachClientHandle {
                master,
                writer,
                child,
            });
        }

        thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let last_heartbeat = Arc::new(Mutex::new(Instant::now()));
            let stalled_reported = Arc::new(AtomicBool::new(false));
            let mut parse_state = ProviderParseState::default();
            let mut last_parse_error_alert_at: Option<Instant> = None;
            let mut last_heartbeat_write = Instant::now();
            let mut last_emit = Instant::now();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let snippet = String::from_utf8_lossy(&buf[..n]).into_owned();
                        process_output_chunk(
                            &snippet,
                            &last_snippet,
                            &output,
                            &last_heartbeat,
                            &stalled_reported,
                            &idle_input_marked,
                            runtime_context.as_ref(),
                            &mut parse_state,
                            &mut last_parse_error_alert_at,
                            &mut last_heartbeat_write,
                            &mut last_emit,
                        );
                    }
                    Err(_) => break,
                }
            }
            attach_active.store(false, Ordering::SeqCst);
            let sessions = supervisor.sessions.lock().unwrap();
            if let Some(handle) = sessions.get(&session_id) {
                let mut attach = handle.attach_client.lock().unwrap();
                *attach = None;
            }
        });

        Ok(())
    }
}

fn spawn_tmux_monitor(
    supervisor: SessionSupervisor,
    session_id: i64,
    tmux_session_name: String,
    _tmux_pane_id: String,
    log_path: PathBuf,
    last_snippet: Arc<Mutex<String>>,
    output: Arc<Mutex<SessionOutputBuffer>>,
    idle_input_marked: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    attach_active: Arc<AtomicBool>,
    runtime_context: Option<RuntimeEventContext>,
) {
    thread::spawn(move || {
        let last_heartbeat = Arc::new(Mutex::new(Instant::now()));
        let stalled_reported = Arc::new(AtomicBool::new(false));
        let mut offset = 0u64;
        let mut parse_state = ProviderParseState::default();
        let mut last_parse_error_alert_at: Option<Instant> = None;
        let mut last_heartbeat_write = Instant::now();
        let mut last_emit = Instant::now();

        loop {
            if stopped.load(Ordering::SeqCst) {
                break;
            }

            if attach_active.load(Ordering::SeqCst) {
                offset = current_file_len(&log_path).unwrap_or(offset);
            } else if let Some(chunk) = read_new_log_chunk(&log_path, &mut offset) {
                process_output_chunk(
                    &chunk,
                    &last_snippet,
                    &output,
                    &last_heartbeat,
                    &stalled_reported,
                    &idle_input_marked,
                    runtime_context.as_ref(),
                    &mut parse_state,
                    &mut last_parse_error_alert_at,
                    &mut last_heartbeat_write,
                    &mut last_emit,
                );
            }

            if let Some(ctx) = runtime_context.as_ref() {
                let elapsed = {
                    let hb = last_heartbeat.lock().unwrap();
                    hb.elapsed()
                };
                let is_opencode = ctx.provider.eq_ignore_ascii_case("opencode");
                if is_opencode
                    && elapsed >= OPENCODE_IDLE_INPUT_THRESHOLD
                    && !idle_input_marked.swap(true, Ordering::SeqCst)
                {
                    let db_write = ctx.db.clone();
                    let app_write = ctx.app.clone();
                    let session_id = ctx.session_id;
                    tauri::async_runtime::spawn(async move {
                        let _ = db_write
                            .mark_session_needs_input(
                                session_id,
                                OPENCODE_IDLE_INPUT_REASON,
                                OPENCODE_IDLE_INPUT_MESSAGE,
                            )
                            .await;
                        emit_runtime_events(&app_write, &db_write, session_id).await;
                    });
                }

                if elapsed > STALE_THRESHOLD
                    && stalled_reported
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    let stalled_guard = Arc::clone(&stalled_reported);
                    let db_write = ctx.db.clone();
                    let app_write = ctx.app.clone();
                    let session_id = ctx.session_id;
                    tauri::async_runtime::spawn(async move {
                        match db_write
                            .mark_session_stalled_if_not_needs_input(session_id)
                            .await
                        {
                            Ok(true) => {
                                let _ = db_write
                                    .insert_session_event(
                                        session_id,
                                        "stalled",
                                        Some("heartbeat stale"),
                                        None,
                                    )
                                    .await;
                                emit_runtime_events(&app_write, &db_write, session_id).await;
                            }
                            Ok(false) | Err(_) => {
                                stalled_guard.store(false, Ordering::SeqCst);
                            }
                        }
                    });
                }
            }

            if !tmux_has_session(&tmux_session_name) {
                if let Some(ctx) = runtime_context.as_ref() {
                    if !stopped.load(Ordering::SeqCst) {
                        let db_write = ctx.db.clone();
                        let app_write = ctx.app.clone();
                        let session_id = ctx.session_id;
                        let agent_id = ctx.agent_id;
                        tauri::async_runtime::spawn(async move {
                            let _ = db_write.end_session(session_id, None).await;
                            let _ = db_write
                                .insert_session_event(session_id, "ended", Some("tmux session ended"), None)
                                .await;
                            record_session_ended_metric(
                                &app_write,
                                session_id,
                                agent_id,
                                "process_exit",
                                "supervisor.tmux_monitor",
                            );
                            emit_runtime_events(&app_write, &db_write, session_id).await;
                        });
                    }
                }
                break;
            }

            thread::sleep(OUTPUT_MONITOR_INTERVAL);
        }

        supervisor.sessions.lock().unwrap().remove(&session_id);
    });
}

fn process_output_chunk(
    snippet: &str,
    last_snippet: &Arc<Mutex<String>>,
    output: &Arc<Mutex<SessionOutputBuffer>>,
    last_heartbeat: &Arc<Mutex<Instant>>,
    stalled_reported: &Arc<AtomicBool>,
    idle_input_marked: &Arc<AtomicBool>,
    runtime_context: Option<&RuntimeEventContext>,
    parse_state: &mut ProviderParseState,
    last_parse_error_alert_at: &mut Option<Instant>,
    last_heartbeat_write: &mut Instant,
    last_emit: &mut Instant,
) {
    if snippet.is_empty() {
        return;
    }

    let snippet_preview = sanitize_terminal_preview(snippet, TERMINAL_SNIPPET_MAX_CHARS);
    if !snippet_preview.is_empty() {
        let mut stored = last_snippet.lock().unwrap();
        *stored = snippet_preview.clone();
    }

    let cursor = {
        let mut out = output.lock().unwrap();
        out.append(snippet)
    };
    {
        let mut hb = last_heartbeat.lock().unwrap();
        *hb = Instant::now();
    }

    let Some(ctx) = runtime_context else {
        return;
    };

    if ctx.provider.eq_ignore_ascii_case("opencode")
        && idle_input_marked.swap(false, Ordering::SeqCst)
    {
        let db_write = ctx.db.clone();
        let app_write = ctx.app.clone();
        let session_id = ctx.session_id;
        tauri::async_runtime::spawn(async move {
            let _ = db_write.clear_session_needs_input(session_id).await;
            emit_runtime_events(&app_write, &db_write, session_id).await;
        });
    }

    let _ = ctx.app.emit(
        "terminal_chunk",
        TerminalChunkEvent {
            session_id: ctx.session_id,
            cursor,
            chunk: snippet.to_string(),
            is_delta: true,
            at: now_timestamp(),
        },
    );

    if stalled_reported.swap(false, Ordering::SeqCst) {
        let db_write = ctx.db.clone();
        let app_write = ctx.app.clone();
        let session_id = ctx.session_id;
        tauri::async_runtime::spawn(async move {
            let _ = db_write
                .update_session_status(session_id, "active", None)
                .await;
            let _ = db_write
                .insert_session_event(session_id, "heartbeat", Some("session recovered"), None)
                .await;
            emit_runtime_events(&app_write, &db_write, session_id).await;
        });
    }

    let adapter = adapter_for(&ctx.provider);
    match adapter.parse_stream_chunk(snippet, parse_state) {
        Ok(events) => {
            for event in events {
                handle_provider_structured_event(
                    event,
                    ctx.db.clone(),
                    ctx.app.clone(),
                    ctx.session_id,
                    ctx.agent_id,
                );
            }
        }
        Err(err) => {
            let should_emit_alert = should_emit_parse_error_alert(last_parse_error_alert_at);
            handle_provider_parse_error(
                err.to_string(),
                ctx.db.clone(),
                ctx.app.clone(),
                ctx.session_id,
                ctx.agent_id,
                should_emit_alert,
            );
        }
    }

    if last_heartbeat_write.elapsed() >= HEARTBEAT_INTERVAL {
        let db_write = ctx.db.clone();
        let session_id = ctx.session_id;
        tauri::async_runtime::spawn(async move {
            let _ = db_write.update_session_heartbeat(session_id).await;
        });
        *last_heartbeat_write = Instant::now();
    }

    if last_emit.elapsed() >= Duration::from_millis(250) {
        let snippet_for_db = snippet_preview;
        if !snippet_for_db.is_empty() {
            let db_write = ctx.db.clone();
            let agent_id = ctx.agent_id;
            tauri::async_runtime::spawn(async move {
                if let Some(agent_id) = agent_id {
                    let _ = db_write.update_agent_snippet(agent_id, &snippet_for_db).await;
                }
            });
        }
        *last_emit = Instant::now();
    }
}

fn read_new_log_chunk(path: &Path, offset: &mut u64) -> Option<String> {
    let len = current_file_len(path).ok()?;
    if len < *offset {
        *offset = 0;
    }
    if len == *offset {
        return None;
    }

    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    *offset = len;
    if buf.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn current_file_len(path: &Path) -> std::io::Result<u64> {
    Ok(fs::metadata(path)?.len())
}

fn parse_tmux_metadata(raw: Option<&str>) -> Option<TmuxSessionMetadata> {
    let raw = raw?;
    serde_json::from_str(raw).ok()
}

fn ensure_tmux_available() -> Result<()> {
    let output = tmux_command()
        .arg("-V")
        .output()
        .context("failed to execute tmux")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "tmux is not installed or not available on PATH: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn tmux_new_session(
    session_name: &str,
    cwd: Option<&str>,
    command: &str,
    args: &[String],
    log_path: &Path,
) -> Result<TmuxCreateResult> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    let mut cmd = Command::new("tmux");
    tmux_configure_command(&mut cmd);
    cmd.args([
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}|#{window_id}",
        "-s",
        session_name,
    ]);
    if let Some(cwd) = cwd {
        cmd.args(["-c", cwd]);
    }
    let output = run_command(cmd)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut parts = stdout.split('|');
    let pane_id = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("tmux did not return a pane id"))?
        .to_string();
    let window_id = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("tmux did not return a window id"))?
        .to_string();

    let pipe_command = format!("cat >> {}", shell_quote(&log_path.to_string_lossy()));
    let output = tmux_command()
        .args(["pipe-pane", "-o", "-t", &pane_id, &pipe_command])
        .output()
        .context("failed to configure tmux pane pipe")?;
    if !output.status.success() {
        let _ = tmux_kill_session(session_name);
        return Err(anyhow!(
            "failed to configure tmux output pipe: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let command_line = build_tmux_shell_command(command, args);
    if let Err(err) = tmux_send_line(&pane_id, &command_line) {
        let _ = tmux_kill_session(session_name);
        return Err(err);
    }

    Ok(TmuxCreateResult { pane_id, window_id })
}

fn build_tmux_shell_command(command: &str, args: &[String]) -> String {
    let mut line = format!("exec {}", shell_quote(command));
    for arg in args {
        line.push(' ');
        line.push_str(&shell_quote(arg));
    }
    line
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn run_command(mut cmd: Command) -> Result<Output> {
    let output = cmd.output()?;
    if output.status.success() {
        return Ok(output);
    }
    Err(anyhow!(
        "{}",
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    ))
}

fn tmux_has_session(session_name: &str) -> bool {
    tmux_command()
        .args(["has-session", "-t", session_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn tmux_kill_session(session_name: &str) -> Result<()> {
    let status = tmux_command()
        .args(["kill-session", "-t", session_name])
        .status()
        .context("failed to execute tmux kill-session")?;
    if status.success() {
        return Ok(());
    }
    if !tmux_has_session(session_name) {
        return Ok(());
    }
    Err(anyhow!("failed to kill tmux session {session_name}"))
}

fn tmux_resize_pane(pane_id: &str, cols: u16, rows: u16) -> Result<()> {
    let output = tmux_command()
        .args([
            "resize-pane",
            "-t",
            pane_id,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ])
        .output()
        .context("failed to execute tmux resize-pane")?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "failed to resize tmux pane: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn tmux_capture_pane(pane_id: &str) -> Result<String> {
    let output = tmux_command()
        .args(["capture-pane", "-p", "-e", "-S", TMUX_CAPTURE_LINES, "-t", pane_id])
        .output()
        .context("failed to capture tmux pane")?;
    if !output.status.success() {
        return Err(anyhow!(
            "failed to capture tmux pane: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn tmux_send_input(pane_id: &str, input: &str) -> Result<()> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.contains('\n') {
        return tmux_send_literal(pane_id, &normalized);
    }

    for segment in normalized.split_inclusive('\n') {
        let line = segment.trim_end_matches('\n');
        if !line.is_empty() {
            tmux_send_literal(pane_id, line)?;
        }
        if segment.ends_with('\n') {
            tmux_send_enter(pane_id)?;
        }
    }

    Ok(())
}

fn tmux_send_line(pane_id: &str, line: &str) -> Result<()> {
    tmux_send_literal(pane_id, line)?;
    tmux_send_enter(pane_id)
}

fn tmux_send_literal(pane_id: &str, text: &str) -> Result<()> {
    let output = tmux_command()
        .args(["send-keys", "-t", pane_id, "-l", text])
        .output()
        .context("failed to execute tmux send-keys")?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "failed to send tmux literal input: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn tmux_send_enter(pane_id: &str) -> Result<()> {
    let output = tmux_command()
        .args(["send-keys", "-t", pane_id, "C-m"])
        .output()
        .context("failed to execute tmux send-keys enter")?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "failed to send tmux enter: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn should_emit_parse_error_alert(last: &mut Option<Instant>) -> bool {
    let should_emit = last
        .map(|value| value.elapsed() >= PARSE_ERROR_ALERT_THROTTLE)
        .unwrap_or(true);
    if should_emit {
        *last = Some(Instant::now());
    }
    should_emit
}

fn handle_provider_parse_error(
    error: String,
    db: Db,
    app: AppHandle,
    session_id: i64,
    agent_id: Option<i64>,
    should_emit_alert: bool,
) {
    let model_endpoint = app.state::<EchoConfig>().model_endpoint.clone();
    tauri::async_runtime::spawn(async move {
        let _ = db
            .insert_session_event(
                session_id,
                "provider_parse_error",
                Some("adapter parse error"),
                Some(&serde_json::json!({ "error": error }).to_string()),
            )
            .await;
        if should_emit_alert {
            let severity = "warning".to_string();
            let reason = "unknown".to_string();
            let message = "Provider parse error; using fallback stream mode".to_string();
            let enrichment = enrich_issue_message(&model_endpoint, &message).await;
            if let Ok(alert) = db
                .create_session_alert_with_enrichment(
                    session_id,
                    agent_id,
                    &severity,
                    &reason,
                    &message,
                    false,
                    AlertEnrichmentInput {
                        message_enriched: enrichment.cleaned.clone(),
                        message_enrichment_status: Some(enrichment.status.clone()),
                        message_enrichment_error: enrichment.error.clone(),
                    },
                )
                .await
            {
                let _ = app.emit(
                    "session_alert_created",
                    SessionAlertCreatedEvent {
                        alert_id: alert.id,
                        session_id,
                        agent_id,
                        severity,
                        reason,
                        message,
                        message_enriched: alert.message_enriched.clone(),
                        message_enrichment_status: alert.message_enrichment_status.clone(),
                        message_enriched_at: alert.message_enriched_at.clone(),
                        requires_ack: false,
                        created_at: alert.created_at,
                    },
                );
            }
            emit_runtime_events(&app, &db, session_id).await;
        }
    });
}

fn handle_provider_structured_event(
    event: ProviderStructuredEvent,
    db: Db,
    app: AppHandle,
    session_id: i64,
    agent_id: Option<i64>,
) {
    let model_endpoint = app.state::<EchoConfig>().model_endpoint.clone();
    match event {
        ProviderStructuredEvent::InputRequired {
            severity,
            reason,
            message,
            requires_ack,
        } => {
            tauri::async_runtime::spawn(async move {
                let structured_payload = serde_json::json!({
                    "severity": severity.clone(),
                    "reason": reason.clone(),
                    "message": message.clone(),
                    "requiresAck": requires_ack
                })
                .to_string();
                let _ = db
                    .insert_session_event(
                        session_id,
                        "provider_structured_alert",
                        Some("provider reported structured alert"),
                        Some(&structured_payload),
                    )
                    .await;
                let _ = db
                    .mark_session_needs_input(session_id, &reason, &message)
                    .await;
                let enrichment = enrich_issue_message(&model_endpoint, &message).await;
                if let Ok(alert) = db
                    .create_session_alert_with_enrichment(
                        session_id,
                        agent_id,
                        &severity,
                        &reason,
                        &message,
                        requires_ack,
                        AlertEnrichmentInput {
                            message_enriched: enrichment.cleaned.clone(),
                            message_enrichment_status: Some(enrichment.status.clone()),
                            message_enrichment_error: enrichment.error.clone(),
                        },
                    )
                    .await
                {
                    let _ = app.emit(
                        "session_alert_created",
                        SessionAlertCreatedEvent {
                            alert_id: alert.id,
                            session_id,
                            agent_id: alert.agent_id,
                            severity: alert.severity.clone(),
                            reason: alert.reason.clone(),
                            message: alert.message.clone(),
                            message_enriched: alert.message_enriched.clone(),
                            message_enrichment_status: alert.message_enrichment_status.clone(),
                            message_enriched_at: alert.message_enriched_at.clone(),
                            requires_ack: alert.requires_ack,
                            created_at: alert.created_at.clone(),
                        },
                    );
                }
                emit_runtime_events(&app, &db, session_id).await;
            });
        }
        ProviderStructuredEvent::SessionStatus { status, reason } => {
            tauri::async_runtime::spawn(async move {
                let structured_payload = serde_json::json!({
                    "status": status.clone(),
                    "reason": reason.clone()
                })
                .to_string();
                let _ = db
                    .insert_session_event(
                        session_id,
                        "provider_structured_status",
                        Some("provider reported structured status"),
                        Some(&structured_payload),
                    )
                    .await;
                let _ = db
                    .update_session_status(session_id, &status, reason.as_deref())
                    .await;
                emit_runtime_events(&app, &db, session_id).await;
            });
        }
    }
}

fn clamp_to_char_boundary(text: &str, idx: usize) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    let mut i = idx;
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(text: &str, idx: usize) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    let mut i = idx + 1;
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i.min(text.len())
}

fn sanitize_terminal_preview(input: &str, max_chars: usize) -> String {
    if input.trim().is_empty() || max_chars == 0 {
        return String::new();
    }

    let stripped = strip_ansi_sequences(input);
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
    truncate_chars(collapsed.trim(), max_chars, true)
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

fn truncate_chars(input: &str, max_chars: usize, keep_tail: bool) -> String {
    if max_chars == 0 || input.is_empty() {
        return String::new();
    }
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }

    if keep_tail {
        if max_chars == 1 {
            return "…".to_string();
        }
        let start = count.saturating_sub(max_chars.saturating_sub(1));
        let tail = input.chars().skip(start).collect::<String>();
        return format!("…{}", tail);
    }

    let head = input
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{}…", head)
}

async fn emit_runtime_events(app: &AppHandle, db: &Db, session_id: i64) {
    if let Ok(session) = db.get_managed_session(session_id).await {
        if let Some(agent_id) = session.agent_id {
            let unresolved_count = db
                .list_unresolved_session_alerts(Some(agent_id), Some(100))
                .await
                .map(|rows| rows.len() as i64)
                .unwrap_or(0);
            if let Ok(agent) = db.get_agent(agent_id).await {
                let _ = app.emit(
                    "agent_runtime_updated",
                    AgentRuntimeUpdatedEvent {
                        agent_id,
                        active_session_id: agent.active_session_id,
                        status: session.status,
                        attention_state: agent.attention_state.clone(),
                        last_activity_at: session.last_activity_at,
                    },
                );
                let _ = app.emit(
                    "agent_attention_updated",
                    AgentAttentionUpdatedEvent {
                        agent_id,
                        attention_state: agent.attention_state,
                        unresolved_alert_count: unresolved_count,
                        last_input_required_at: agent.last_input_required_at,
                    },
                );
            }
        }
    }
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn record_session_started_metric(
    app: &AppHandle,
    session_id: i64,
    agent_id: Option<i64>,
    provider: &str,
    source: &str,
) {
    let telemetry = app.state::<Telemetry>();
    telemetry.record_session_started(session_id, agent_id, provider, source);
}

fn record_session_start_failed_metric(
    app: &AppHandle,
    session_id: Option<i64>,
    agent_id: Option<i64>,
    provider: &str,
    source: &str,
    reason: &str,
) {
    let telemetry = app.state::<Telemetry>();
    telemetry.record_session_start_failed(session_id, agent_id, provider, source, reason);
}

fn record_session_ended_metric(
    app: &AppHandle,
    session_id: i64,
    agent_id: Option<i64>,
    reason: &str,
    source: &str,
) {
    let telemetry = app.state::<Telemetry>();
    telemetry.record_session_ended(session_id, agent_id, reason, source);
}

fn tmux_tmpdir() -> PathBuf {
    let base = PathBuf::from("/tmp").join("echo-tmux");
    let _ = fs::create_dir_all(&base);
    base
}

fn tmux_command() -> Command {
    let mut cmd = Command::new("tmux");
    tmux_configure_command(&mut cmd);
    cmd
}

fn tmux_configure_command(cmd: &mut Command) {
    cmd.env("TMUX", "");
    cmd.arg("-S");
    cmd.arg(tmux_socket_path());
}

fn tmux_socket_path() -> PathBuf {
    tmux_tmpdir().join(TMUX_SOCKET_FILE)
}

#[cfg(test)]
mod tests;
