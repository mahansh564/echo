use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::db::{models::ManagedSession, models::StartSessionRequest, Db};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const STALE_THRESHOLD: Duration = Duration::from_secs(20);
const MONITOR_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<i64, SessionHandle>>>,
    #[cfg(test)]
    next_legacy_id: Arc<AtomicU64>,
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
            sessions: Arc::new(Mutex::new(HashMap::new())),
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
        if request.command.trim().is_empty() {
            return Err(anyhow!("command is required"));
        }

        let provider = request.provider.clone().unwrap_or_else(|| "opencode".to_string());
        let args_json = serde_json::to_string(&request.args)?;
        let managed = tauri::async_runtime::block_on(db.create_managed_session(
            &provider,
            request.command.trim(),
            &args_json,
            request.cwd.as_deref(),
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
                });
                return Err(err.into());
            }
        };

        let mut cmd = CommandBuilder::new(request.command.trim());
        if !request.args.is_empty() {
            cmd.args(request.args.iter().map(String::as_str).collect::<Vec<_>>());
        }
        if let Some(cwd) = request.cwd.as_deref() {
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
                            "command": request.command,
                            "args": request.args,
                            "cwd": request.cwd,
                            "pid": pid,
                        })
                        .to_string(),
                    ),
                )
                .await;
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

        let manager_for_reader = self.clone();
        let app_handle = app.clone();
        let db_for_reader = db.clone();
        let agent_id = request.agent_id;
        let session_id = managed.id;
        let stopped_for_reader = Arc::clone(&stopped);
        let hb_for_reader = Arc::clone(&last_heartbeat);
        let stalled_for_reader = Arc::clone(&stalled_reported);
        let output_for_reader = Arc::clone(&output);
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut last_emit = Instant::now();
            let mut last_heartbeat_write = Instant::now();

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
                                    let _ = db_write.update_session_status(session_id, "active", None).await;
                                    let _ = db_write.insert_session_event(session_id, "heartbeat", Some("session recovered"), None).await;
                                    if let Ok(row) = db_write.get_managed_session(session_id).await {
                                        let _ = app_write.emit("managed_session_updated", ManagedSessionUpdatedEvent {
                                            session_id,
                                            status: row.status,
                                            last_heartbeat_at: row.last_heartbeat_at,
                                            agent_id: row.agent_id,
                                            task_id: row.task_id,
                                        });
                                    }
                                });
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
                                        let _ = db_write.update_agent_snippet(agent_id, &snippet).await;
                                    }
                                    let _ = db_write
                                        .insert_session_event(
                                            session_id,
                                            "snippet",
                                            None,
                                            Some(&serde_json::json!({ "snippet": snippet }).to_string()),
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
                    let _ = db_write.insert_session_event(session_id, "ended", Some("session ended"), None).await;
                    if let Ok(row) = db_write.get_managed_session(session_id).await {
                        let _ = app_write.emit("managed_session_updated", ManagedSessionUpdatedEvent {
                            session_id,
                            status: row.status,
                            last_heartbeat_at: row.last_heartbeat_at,
                            agent_id: row.agent_id,
                            task_id: row.task_id,
                        });
                    }
                });
            }
            manager_for_reader.sessions.lock().unwrap().remove(&session_id);
        });

        let db_for_monitor = db.clone();
        let app_for_monitor = app.clone();
        let stopped_for_monitor = Arc::clone(&stopped);
        let hb_for_monitor = Arc::clone(&last_heartbeat);
        let stalled_for_monitor = Arc::clone(&stalled_reported);
        thread::spawn(move || {
            loop {
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
                        let _ = db_write.update_session_status(session_id, "stalled", None).await;
                        let _ = db_write
                            .insert_session_event(session_id, "stalled", Some("heartbeat stale"), None)
                            .await;
                        if let Ok(row) = db_write.get_managed_session(session_id).await {
                            let _ = app_write.emit("managed_session_updated", ManagedSessionUpdatedEvent {
                                session_id,
                                status: row.status,
                                last_heartbeat_at: row.last_heartbeat_at,
                                agent_id: row.agent_id,
                                task_id: row.task_id,
                            });
                        }
                    });
                }
            }
        });

        let mut latest = tauri::async_runtime::block_on(db.get_managed_session(managed.id))?;
        latest.status = "active".to_string();
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
        let mut writer = handle.writer.lock().map_err(|_| anyhow!("writer lock poisoned"))?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
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
            command: format!("{} -l", std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())),
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

        self.sessions.lock().unwrap().insert(id as i64, handle);

        Ok(TerminalSession {
            id,
            agent_id: 0,
            command: format!("{} {}", command, args.join(" ")),
        })
    }
}

#[cfg(test)]
mod tests;
