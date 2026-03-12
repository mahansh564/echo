use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

use crate::db::{models::ManagedSession, models::StartSessionRequest, Db};
use crate::providers::{adapter_for, ProviderParseState, ProviderStructuredEvent};
use crate::telemetry::Telemetry;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const STALE_THRESHOLD: Duration = Duration::from_secs(20);
const PARSE_ERROR_ALERT_THROTTLE: Duration = Duration::from_secs(30);
const OPENCODE_IDLE_INPUT_THRESHOLD: Duration = Duration::from_secs(2);
const OPENCODE_IDLE_INPUT_REASON: &str = "idle_no_output";
const OPENCODE_IDLE_INPUT_MESSAGE: &str = "Waiting for your input";
const IDLE_INPUT_MONITOR_INTERVAL: Duration = Duration::from_millis(250);
const TERMINAL_SNIPPET_MAX_CHARS: usize = 220;
const PER_SESSION_OUTPUT_MAX_BYTES: usize = 200_000;
const TARGET_ACTIVE_SESSION_CAPACITY: usize = 10;
const TOTAL_OUTPUT_MEMORY_CAP_BYTES: usize =
    PER_SESSION_OUTPUT_MAX_BYTES * TARGET_ACTIVE_SESSION_CAPACITY;

fn apply_terminal_env(cmd: &mut CommandBuilder) {
    // Full-screen TUIs break badly with TERM=dumb (partial renders, no alt-screen semantics).
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
    requires_ack: bool,
    created_at: String,
}

struct SessionHandle {
    last_snippet: Arc<Mutex<String>>,
    output: Arc<Mutex<SessionOutputBuffer>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    idle_input_marked: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

#[derive(Clone)]
struct SessionOutputBuffer {
    base_offset: usize,
    data: String,
    max_bytes: usize,
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

    pub fn resize_session(&self, session_id: i64, cols: u16, rows: u16) -> Result<()> {
        self.supervisor.resize_session(session_id, cols, rows)
    }

    pub async fn reconcile_orphan_sessions(&self, db: &Db) -> Result<usize> {
        self.supervisor.reconcile_orphan_sessions(db).await
    }

    // Deprecated compatibility shim.
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

    // Deprecated compatibility shim.
    pub fn stop_session_legacy(&self, db: Db, session_id: u64) -> Result<()> {
        self.stop_session(db, session_id as i64)
    }

    #[cfg(test)]
    pub fn start_session_for_test(&self, command: &str, args: &[&str]) -> Result<TerminalSession> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        apply_terminal_env(&mut cmd);
        let child = pair.slave.spawn_command(cmd)?;
        let master = pair.master;
        let mut reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;

        let id = self.next_legacy_id.fetch_add(1, Ordering::SeqCst);
        let last_snippet = Arc::new(Mutex::new(String::new()));
        let snippet_clone = Arc::clone(&last_snippet);
        let output = Arc::new(Mutex::new(SessionOutputBuffer::with_limit(
            PER_SESSION_OUTPUT_MAX_BYTES,
        )));
        let output_clone = Arc::clone(&output);

        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                        let preview = sanitize_terminal_preview(&text, TERMINAL_SNIPPET_MAX_CHARS);
                        {
                            if !preview.is_empty() {
                                let mut stored = snippet_clone.lock().unwrap();
                                *stored = preview;
                            }
                        }
                        let mut out = output_clone.lock().unwrap();
                        out.append(&text);
                    }
                    Err(_) => break,
                }
            }
        });

        let handle = SessionHandle {
            last_snippet,
            output,
            master: Mutex::new(master),
            writer: Mutex::new(writer),
            idle_input_marked: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(false)),
            child: Mutex::new(child),
        };

        self.supervisor
            .sessions
            .lock()
            .unwrap()
            .insert(id as i64, handle);

        Ok(TerminalSession {
            id,
            agent_id: 0,
            command: format!("{} {}", command, args.join(" ")),
        })
    }
}

impl SessionSupervisor {
    async fn reconcile_orphan_sessions(&self, db: &Db) -> Result<usize> {
        let runtime_sessions: HashSet<i64> = self
            .sessions
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        let rows = db.list_managed_sessions(None, Some(500)).await?;
        let mut reconciled = 0usize;

        for row in rows {
            let is_runtime = runtime_sessions.contains(&row.id);
            let is_open = matches!(
                row.status.as_str(),
                "waking" | "active" | "stalled" | "needs_input"
            );
            if is_open && !is_runtime {
                db.update_session_status(
                    row.id,
                    "failed",
                    Some("orphaned runtime session reconciled on startup"),
                )
                .await?;
                db.insert_session_event(
                    row.id,
                    "orphan_cleanup",
                    Some("session marked failed during startup reconciliation"),
                    None,
                )
                .await?;
                reconciled += 1;
            }
        }

        Ok(reconciled)
    }

    pub async fn start_session(
        &self,
        app: &AppHandle,
        db: Db,
        request: StartSessionRequest,
    ) -> Result<ManagedSession> {
        if let Some(agent_id) = request.agent_id {
            if let Ok(agent) = db.get_agent(agent_id).await {
                if let Some(previous_session_id) = agent.active_session_id {
                    let _ = self.stop_session(db.clone(), previous_session_id);
                }
            }
        }

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

        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(err) => {
                let _ = db
                    .update_session_status(managed.id, "failed", Some(&err.to_string()))
                    .await;
                let _ = db
                    .insert_session_event(
                        managed.id,
                        "error",
                        Some("failed to open PTY"),
                        Some(&serde_json::json!({ "error": err.to_string() }).to_string()),
                    )
                    .await;
                emit_runtime_events(app, &db, managed.id).await;
                record_session_start_failed_metric(
                    app,
                    Some(managed.id),
                    request.agent_id,
                    adapter.provider_name(),
                    "supervisor.open_pty",
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };

        let mut cmd = CommandBuilder::new(&spawn_spec.command);
        if !spawn_spec.args.is_empty() {
            cmd.args(
                spawn_spec
                    .args
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            );
        }
        if let Some(cwd) = spawn_spec.cwd.as_deref() {
            cmd.cwd(cwd);
        }
        apply_terminal_env(&mut cmd);

        let child = match pair.slave.spawn_command(cmd) {
            Ok(child) => child,
            Err(err) => {
                let _ = db
                    .update_session_status(managed.id, "failed", Some(&err.to_string()))
                    .await;
                let _ = db
                    .insert_session_event(
                        managed.id,
                        "error",
                        Some("failed to spawn command"),
                        Some(&serde_json::json!({ "error": err.to_string() }).to_string()),
                    )
                    .await;
                emit_runtime_events(app, &db, managed.id).await;
                record_session_start_failed_metric(
                    app,
                    Some(managed.id),
                    request.agent_id,
                    adapter.provider_name(),
                    "supervisor.spawn_command",
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };

        let pid = child.process_id().map(|p| p as i64);
        let _ = db.set_session_pid(managed.id, pid).await;
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
                        "pid": pid,
                        "provider": adapter.provider_name(),
                        "supports_attach": adapter.supports_terminal_attach(),
                    })
                    .to_string(),
                ),
            )
            .await;
        emit_runtime_events(app, &db, managed.id).await;

        let master = pair.master;
        let mut reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;
        let last_snippet = Arc::new(Mutex::new(String::new()));
        debug_assert!(
            TOTAL_OUTPUT_MEMORY_CAP_BYTES
                >= PER_SESSION_OUTPUT_MAX_BYTES * TARGET_ACTIVE_SESSION_CAPACITY
        );
        let output = Arc::new(Mutex::new(SessionOutputBuffer::with_limit(
            PER_SESSION_OUTPUT_MAX_BYTES,
        )));
        let last_heartbeat = Arc::new(Mutex::new(Instant::now()));
        let stalled_reported = Arc::new(AtomicBool::new(false));
        let idle_input_marked = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));

        let handle = SessionHandle {
            last_snippet: Arc::clone(&last_snippet),
            output: Arc::clone(&output),
            master: Mutex::new(master),
            writer: Mutex::new(writer),
            idle_input_marked: Arc::clone(&idle_input_marked),
            stopped: Arc::clone(&stopped),
            child: Mutex::new(child),
        };

        self.sessions.lock().unwrap().insert(managed.id, handle);

        let supervisor_for_reader = self.clone();
        let app_handle = app.clone();
        let db_for_reader = db.clone();
        let agent_id = request.agent_id;
        let session_id = managed.id;
        let stopped_for_reader = Arc::clone(&stopped);
        let hb_for_reader = Arc::clone(&last_heartbeat);
        let stalled_for_reader = Arc::clone(&stalled_reported);
        let idle_input_for_reader = Arc::clone(&idle_input_marked);
        let output_for_reader = Arc::clone(&output);
        let provider_for_reader = requested_provider.clone();
        let is_opencode_for_reader = provider_for_reader.eq_ignore_ascii_case("opencode");
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut last_emit = Instant::now();
            let mut last_heartbeat_write = Instant::now();
            let adapter = adapter_for(&provider_for_reader);
            let mut parse_state = ProviderParseState::default();
            let mut last_parse_error_alert_at: Option<Instant> = None;

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let snippet = String::from_utf8_lossy(&buf[..n]).into_owned();
                        let snippet_preview =
                            sanitize_terminal_preview(&snippet, TERMINAL_SNIPPET_MAX_CHARS);
                        {
                            if !snippet_preview.is_empty() {
                                let mut stored = last_snippet.lock().unwrap();
                                *stored = snippet_preview.clone();
                            }
                        }
                        let cursor = {
                            let mut out = output_for_reader.lock().unwrap();
                            out.append(&snippet)
                        };
                        {
                            let mut hb = hb_for_reader.lock().unwrap();
                            *hb = Instant::now();
                        }
                        if is_opencode_for_reader
                            && idle_input_for_reader.swap(false, Ordering::SeqCst)
                        {
                            let db_write = db_for_reader.clone();
                            let app_write = app_handle.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = db_write.clear_session_needs_input(session_id).await;
                                emit_runtime_events(&app_write, &db_write, session_id).await;
                            });
                        }

                        let _ = app_handle.emit(
                            "terminal_chunk",
                            TerminalChunkEvent {
                                session_id,
                                cursor,
                                chunk: snippet.clone(),
                                is_delta: true,
                                at: now_timestamp(),
                            },
                        );

                        if stalled_for_reader.swap(false, Ordering::SeqCst) {
                            let db_write = db_for_reader.clone();
                            let app_write = app_handle.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = db_write
                                    .update_session_status(session_id, "active", None)
                                    .await;
                                let _ = db_write
                                    .insert_session_event(
                                        session_id,
                                        "heartbeat",
                                        Some("session recovered"),
                                        None,
                                    )
                                    .await;
                                emit_runtime_events(&app_write, &db_write, session_id).await;
                            });
                        }

                        match adapter.parse_stream_chunk(&snippet, &mut parse_state) {
                            Ok(events) => {
                                for event in events {
                                    handle_provider_structured_event(
                                        event,
                                        db_for_reader.clone(),
                                        app_handle.clone(),
                                        session_id,
                                        agent_id,
                                    );
                                }
                            }
                            Err(err) => {
                                let should_emit_alert =
                                    should_emit_parse_error_alert(&mut last_parse_error_alert_at);
                                handle_provider_parse_error(
                                    err.to_string(),
                                    db_for_reader.clone(),
                                    app_handle.clone(),
                                    session_id,
                                    agent_id,
                                    should_emit_alert,
                                );
                            }
                        }

                        if last_heartbeat_write.elapsed() >= HEARTBEAT_INTERVAL {
                            let db_write = db_for_reader.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = db_write.update_session_heartbeat(session_id).await;
                            });
                            last_heartbeat_write = Instant::now();
                        }

                        if last_emit.elapsed() >= Duration::from_millis(250) {
                            let snippet_for_db = snippet_preview.clone();
                            if !snippet_for_db.is_empty() {
                                let db_write = db_for_reader.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Some(agent_id) = agent_id {
                                        let _ = db_write
                                            .update_agent_snippet(agent_id, &snippet_for_db)
                                            .await;
                                    }
                                });
                            }
                            last_emit = Instant::now();
                        }
                    }
                    Err(_) => break,
                }
            }

            match adapter.flush_stream(&mut parse_state) {
                Ok(events) => {
                    for event in events {
                        handle_provider_structured_event(
                            event,
                            db_for_reader.clone(),
                            app_handle.clone(),
                            session_id,
                            agent_id,
                        );
                    }
                }
                Err(err) => {
                    let should_emit_alert =
                        should_emit_parse_error_alert(&mut last_parse_error_alert_at);
                    handle_provider_parse_error(
                        err.to_string(),
                        db_for_reader.clone(),
                        app_handle.clone(),
                        session_id,
                        agent_id,
                        should_emit_alert,
                    );
                }
            }

            if !stopped_for_reader.load(Ordering::SeqCst) {
                let db_write = db_for_reader.clone();
                let app_write = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = db_write.end_session(session_id, None).await;
                    let _ = db_write
                        .insert_session_event(session_id, "ended", Some("session ended"), None)
                        .await;
                    record_session_ended_metric(
                        &app_write,
                        session_id,
                        agent_id,
                        "process_exit",
                        "supervisor.reader",
                    );
                    emit_runtime_events(&app_write, &db_write, session_id).await;
                });
            }
            supervisor_for_reader
                .sessions
                .lock()
                .unwrap()
                .remove(&session_id);
        });

        let db_for_monitor = db.clone();
        let app_for_monitor = app.clone();
        let stopped_for_monitor = Arc::clone(&stopped);
        let hb_for_monitor = Arc::clone(&last_heartbeat);
        let stalled_for_monitor = Arc::clone(&stalled_reported);
        let idle_input_for_monitor = Arc::clone(&idle_input_marked);
        let is_opencode_for_monitor = requested_provider.eq_ignore_ascii_case("opencode");
        thread::spawn(move || loop {
            if stopped_for_monitor.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(IDLE_INPUT_MONITOR_INTERVAL);
            let elapsed = {
                let hb = hb_for_monitor.lock().unwrap();
                hb.elapsed()
            };
            if is_opencode_for_monitor
                && elapsed >= OPENCODE_IDLE_INPUT_THRESHOLD
                && !idle_input_for_monitor.swap(true, Ordering::SeqCst)
            {
                let db_write = db_for_monitor.clone();
                let app_write = app_for_monitor.clone();
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
                && stalled_for_monitor
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                let stalled_guard = Arc::clone(&stalled_for_monitor);
                let db_write = db_for_monitor.clone();
                let app_write = app_for_monitor.clone();
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
        });

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
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(handle) = sessions.remove(&session_id) {
            handle.stopped.store(true, Ordering::SeqCst);
            if let Ok(mut child) = handle.child.lock() {
                let _ = child.kill();
            }
        }
        drop(sessions);

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

        let updated = db.attach_terminal_session(session_id).await?;
        if !self.has_session(session_id) {
            let _ = db.detach_terminal_session(session_id).await;
            return Err(anyhow!("session ended before attach completed"));
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
        let mut writer = handle
            .writer
            .lock()
            .map_err(|_| anyhow!("writer lock poisoned"))?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize_session(&self, session_id: i64, cols: u16, rows: u16) -> Result<()> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow!("session not found"))?;
        let master = handle
            .master
            .lock()
            .map_err(|_| anyhow!("master lock poisoned"))?;
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }
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
            if let Ok(alert) = db
                .create_session_alert(session_id, agent_id, &severity, &reason, &message, false)
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
                        requires_ack: false,
                        created_at: alert.created_at,
                    },
                );
            }
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
                if let Ok(alert) = db
                    .create_session_alert(
                        session_id,
                        agent_id,
                        &severity,
                        &reason,
                        &message,
                        requires_ack,
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
                        active_session_id: Some(session_id),
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

#[cfg(test)]
mod tests;
