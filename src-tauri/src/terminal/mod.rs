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
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::db::{models::ManagedSession, models::StartSessionRequest, Db};
use crate::providers::{adapter_for, ProviderStructuredEvent};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const STALE_THRESHOLD: Duration = Duration::from_secs(20);
const MONITOR_INTERVAL: Duration = Duration::from_secs(5);
const PARSE_ERROR_ALERT_THROTTLE: Duration = Duration::from_secs(30);

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
struct TerminalSnippetEvent {
    agent_id: Option<i64>,
    session_id: i64,
    snippet: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ManagedSessionUpdatedEvent {
    session_id: i64,
    status: String,
    last_heartbeat_at: Option<String>,
    agent_id: Option<i64>,
    task_id: Option<i64>,
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
    output: Arc<Mutex<String>>,
    writer: Mutex<Box<dyn Write + Send>>,
    stopped: Arc<AtomicBool>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
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

    pub fn start_session(
        &self,
        app: &AppHandle,
        db: Db,
        request: StartSessionRequest,
    ) -> Result<ManagedSession> {
        self.supervisor.start_session(app, db, request)
    }

    pub fn stop_session(&self, db: Db, session_id: i64) -> Result<()> {
        self.supervisor.stop_session(db, session_id)
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

    pub fn send_input(&self, session_id: i64, input: &str) -> Result<()> {
        self.supervisor.send_input(session_id, input)
    }

    pub async fn reconcile_orphan_sessions(&self, db: &Db) -> Result<usize> {
        self.supervisor.reconcile_orphan_sessions(db).await
    }

    // Deprecated compatibility shim.
    pub fn start_session_legacy(
        &self,
        app: &AppHandle,
        db: Db,
        agent_id: i64,
    ) -> Result<TerminalSession> {
        let session = self.start_session(
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
        )?;
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
        let child = pair.slave.spawn_command(cmd)?;
        let mut reader = pair.master.try_clone_reader()?;

        let id = self.next_legacy_id.fetch_add(1, Ordering::SeqCst);
        let last_snippet = Arc::new(Mutex::new(String::new()));
        let snippet_clone = Arc::clone(&last_snippet);

        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                            let mut stored = snippet_clone.lock().unwrap();
                            *stored = text.to_string();
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let handle = SessionHandle {
            last_snippet,
            output: Arc::new(Mutex::new(String::new())),
            writer: Mutex::new(pair.master.take_writer()?),
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

    pub fn start_session(
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
        let spawn_spec = adapter.spawn_session(&request)?;
        if spawn_spec.command.trim().is_empty() {
            return Err(anyhow!("command is required"));
        }

        let args_json = serde_json::to_string(&spawn_spec.args)?;
        let managed = tauri::async_runtime::block_on(db.create_managed_session(
            adapter.provider_name(),
            &spawn_spec.command,
            &args_json,
            spawn_spec.cwd.as_deref(),
            request.agent_id,
            request.task_id,
            None,
        ))?;

        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(err) => {
                tauri::async_runtime::block_on(async {
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
                });
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

        let child = match pair.slave.spawn_command(cmd) {
            Ok(child) => child,
            Err(err) => {
                tauri::async_runtime::block_on(async {
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
                });
                return Err(err.into());
            }
        };

        let pid = child.process_id().map(|p| p as i64);
        tauri::async_runtime::block_on(async {
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
        });

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let last_snippet = Arc::new(Mutex::new(String::new()));
        let output = Arc::new(Mutex::new(String::new()));
        let last_heartbeat = Arc::new(Mutex::new(Instant::now()));
        let stalled_reported = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));

        let handle = SessionHandle {
            last_snippet: Arc::clone(&last_snippet),
            output: Arc::clone(&output),
            writer: Mutex::new(writer),
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
        let output_for_reader = Arc::clone(&output);
        let provider_for_reader = requested_provider.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut last_emit = Instant::now();
            let mut last_heartbeat_write = Instant::now();
            let adapter = adapter_for(&provider_for_reader);
            let mut last_parse_error_alert_at: Option<Instant> = None;

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                            let snippet = text.to_string();
                            {
                                let mut stored = last_snippet.lock().unwrap();
                                *stored = snippet.clone();
                            }
                            {
                                let mut out = output_for_reader.lock().unwrap();
                                out.push_str(&snippet);
                                if out.len() > 500_000 {
                                    let trim_at = out.len() - 500_000;
                                    out.drain(..trim_at);
                                }
                            }
                            {
                                let mut hb = hb_for_reader.lock().unwrap();
                                *hb = Instant::now();
                            }

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

                            match adapter.parse_structured_event(&snippet) {
                                Ok(Some(ProviderStructuredEvent::InputRequired {
                                    severity,
                                    reason,
                                    message,
                                    requires_ack,
                                })) => {
                                    let db_write = db_for_reader.clone();
                                    let app_write = app_handle.clone();
                                    tauri::async_runtime::spawn(async move {
                                        let _ = db_write
                                            .mark_session_needs_input(session_id, &reason, &message)
                                            .await;
                                        if let Ok(alert) = db_write
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
                                            let _ = app_write.emit(
                                                "session_alert_created",
                                                SessionAlertCreatedEvent {
                                                    alert_id: alert.id,
                                                    session_id,
                                                    agent_id,
                                                    severity,
                                                    reason,
                                                    message,
                                                    requires_ack,
                                                    created_at: alert.created_at,
                                                },
                                            );
                                        }
                                        emit_runtime_events(&app_write, &db_write, session_id)
                                            .await;
                                    });
                                }
                                Ok(Some(ProviderStructuredEvent::SessionStatus {
                                    status,
                                    reason,
                                })) => {
                                    let db_write = db_for_reader.clone();
                                    let app_write = app_handle.clone();
                                    tauri::async_runtime::spawn(async move {
                                        let _ = db_write
                                            .update_session_status(
                                                session_id,
                                                &status,
                                                reason.as_deref(),
                                            )
                                            .await;
                                        emit_runtime_events(&app_write, &db_write, session_id)
                                            .await;
                                    });
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    let db_write = db_for_reader.clone();
                                    let app_write = app_handle.clone();
                                    let should_emit_alert = last_parse_error_alert_at
                                        .map(|last| last.elapsed() >= PARSE_ERROR_ALERT_THROTTLE)
                                        .unwrap_or(true);
                                    if should_emit_alert {
                                        last_parse_error_alert_at = Some(Instant::now());
                                    }
                                    tauri::async_runtime::spawn(async move {
                                        let _ = db_write
                                            .insert_session_event(
                                                session_id,
                                                "provider_parse_error",
                                                Some("adapter parse error"),
                                                Some(
                                                    &serde_json::json!({ "error": err.to_string() })
                                                        .to_string(),
                                                ),
                                            )
                                            .await;
                                        if should_emit_alert {
                                            let severity = "warning".to_string();
                                            let reason = "unknown".to_string();
                                            let message =
                                                "Provider parse error; using fallback stream mode"
                                                    .to_string();
                                            if let Ok(alert) = db_write
                                                .create_session_alert(
                                                    session_id, agent_id, &severity, &reason,
                                                    &message, false,
                                                )
                                                .await
                                            {
                                                let _ = app_write.emit(
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
                            }

                            if last_heartbeat_write.elapsed() >= HEARTBEAT_INTERVAL {
                                let db_write = db_for_reader.clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ = db_write.update_session_heartbeat(session_id).await;
                                });
                                last_heartbeat_write = Instant::now();
                            }

                            if last_emit.elapsed() >= Duration::from_millis(250) {
                                let payload = TerminalSnippetEvent {
                                    agent_id,
                                    session_id,
                                    snippet: snippet.clone(),
                                };
                                let _ = app_handle.emit("terminal_snippet_updated", payload);
                                let db_write = db_for_reader.clone();
                                tauri::async_runtime::spawn(async move {
                                    if let Some(agent_id) = agent_id {
                                        let _ =
                                            db_write.update_agent_snippet(agent_id, &snippet).await;
                                    }
                                    let _ = db_write
                                        .insert_session_event(
                                            session_id,
                                            "snippet",
                                            None,
                                            Some(
                                                &serde_json::json!({ "snippet": snippet })
                                                    .to_string(),
                                            ),
                                        )
                                        .await;
                                });
                                last_emit = Instant::now();
                            }
                        }
                    }
                    Err(_) => break,
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
        thread::spawn(move || loop {
            if stopped_for_monitor.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(MONITOR_INTERVAL);
            let elapsed = {
                let hb = hb_for_monitor.lock().unwrap();
                hb.elapsed()
            };
            if elapsed > STALE_THRESHOLD && !stalled_for_monitor.swap(true, Ordering::SeqCst) {
                let db_write = db_for_monitor.clone();
                let app_write = app_for_monitor.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = db_write
                        .update_session_status(session_id, "stalled", None)
                        .await;
                    let _ = db_write
                        .insert_session_event(session_id, "stalled", Some("heartbeat stale"), None)
                        .await;
                    emit_runtime_events(&app_write, &db_write, session_id).await;
                });
            }
        });

        let latest = tauri::async_runtime::block_on(db.get_managed_session(managed.id))?;
        Ok(latest)
    }

    pub fn stop_session(&self, db: Db, session_id: i64) -> Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .remove(&session_id)
            .ok_or_else(|| anyhow!("session not found"))?;

        handle.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut child) = handle.child.lock() {
            let _ = child.kill();
        }

        tauri::async_runtime::spawn(async move {
            let _ = db.end_session(session_id, Some("stopped by user")).await;
            let _ = db
                .insert_session_event(session_id, "ended", Some("stopped by user"), None)
                .await;
        });

        Ok(())
    }

    pub fn last_snippet(&self, session_id: i64) -> Option<String> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions.get(&session_id)?;
        let snippet = handle.last_snippet.lock().unwrap().clone();
        Some(snippet)
    }

    pub fn list_runtime_sessions(&self) -> Vec<i64> {
        let sessions = self.sessions.lock().unwrap();
        sessions.keys().copied().collect()
    }

    pub fn session_output(&self, session_id: i64) -> Option<String> {
        let sessions = self.sessions.lock().unwrap();
        let output = Arc::clone(&sessions.get(&session_id)?.output);
        drop(sessions);
        let text = output.lock().unwrap().clone();
        Some(text)
    }

    pub fn send_input(&self, session_id: i64, input: &str) -> Result<()> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(&session_id)
            .ok_or_else(|| anyhow!("session not found"))?;
        let mut writer = handle
            .writer
            .lock()
            .map_err(|_| anyhow!("writer lock poisoned"))?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }
}

async fn emit_runtime_events(app: &AppHandle, db: &Db, session_id: i64) {
    if let Ok(session) = db.get_managed_session(session_id).await {
        let _ = app.emit(
            "managed_session_updated",
            ManagedSessionUpdatedEvent {
                session_id,
                status: session.status.clone(),
                last_heartbeat_at: session.last_heartbeat_at.clone(),
                agent_id: session.agent_id,
                task_id: session.task_id,
            },
        );

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

#[cfg(test)]
mod tests;
