use crate::commands::emit_agent_updated;
use crate::db::models::{ManagedSession, SessionEvent, StartSessionRequest};
use crate::db::Db;
use crate::telemetry::Telemetry;
use crate::terminal::TerminalManager;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProfile {
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub provider: Option<String>,
    pub task_id: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputChunk {
    pub session_id: i64,
    pub chunk: String,
    pub cursor: usize,
    pub has_more: bool,
    pub is_delta: bool,
    pub at: String,
}

#[tauri::command]
pub async fn start_agent_session_cmd(
    app: tauri::AppHandle,
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    agent_id: i64,
    launch_profile: Option<LaunchProfile>,
) -> Result<ManagedSession, String> {
    let agent = db.get_agent(agent_id).await.map_err(|e| e.to_string())?;
    let profile = launch_profile.unwrap_or(LaunchProfile {
        command: None,
        args: Vec::new(),
        cwd: None,
        provider: None,
        task_id: None,
    });

    let provider = profile.provider.unwrap_or_else(|| agent.provider.clone());
    let command = profile
        .command
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_default();

    let request = StartSessionRequest {
        command,
        args: profile.args,
        cwd: profile.cwd,
        agent_id: Some(agent_id),
        task_id: profile.task_id.or(agent.task_id),
        provider: Some(provider),
    };

    terminal
        .start_session(&app, db.inner().clone(), request)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_agent_session_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    telemetry: tauri::State<'_, Telemetry>,
    session_id: i64,
) -> Result<(), String> {
    let agent_id = db
        .get_managed_session(session_id)
        .await
        .ok()
        .and_then(|session| session.agent_id);
    match terminal.stop_session(db.inner().clone(), session_id) {
        Ok(()) => {
            telemetry.record_session_user_stop(session_id, agent_id, "stop_agent_session_cmd");
            Ok(())
        }
        Err(err) => {
            telemetry.record_session_stop_failed(
                session_id,
                "stop_agent_session_cmd",
                &err.to_string(),
            );
            Err(err.to_string())
        }
    }
}

#[tauri::command]
pub async fn delete_managed_session_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    telemetry: tauri::State<'_, Telemetry>,
    session_id: i64,
) -> Result<(), String> {
    let agent_id = db
        .get_managed_session(session_id)
        .await
        .ok()
        .and_then(|session| session.agent_id);
    if terminal.has_session(session_id) {
        if let Err(err) = terminal.stop_session(db.inner().clone(), session_id) {
            telemetry.record_session_stop_failed(
                session_id,
                "delete_managed_session_cmd",
                &err.to_string(),
            );
            return Err(err.to_string());
        }
        telemetry.record_session_user_stop(session_id, agent_id, "delete_managed_session_cmd");
    }
    db.delete_managed_session(session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_managed_sessions_cmd(
    db: tauri::State<'_, Db>,
    status: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<ManagedSession>, String> {
    db.list_managed_sessions(status.as_deref(), limit)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_session_events_cmd(
    db: tauri::State<'_, Db>,
    session_id: i64,
    limit: Option<i64>,
) -> Result<Vec<SessionEvent>, String> {
    db.list_session_events(session_id, limit)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_terminal_snippet_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    session_id: i64,
) -> Result<String, String> {
    Ok(terminal.last_snippet(session_id).unwrap_or_default())
}

#[tauri::command]
pub async fn get_terminal_output_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    session_id: i64,
    cursor: Option<usize>,
    max_bytes: Option<usize>,
) -> Result<TerminalOutputChunk, String> {
    let initial_cursor = cursor.unwrap_or(0);
    let chunk_size = max_bytes.unwrap_or(16_384).clamp(256, 65_536);
    let (chunk, next_cursor, has_more) = terminal
        .session_output_chunk(session_id, initial_cursor, chunk_size)
        .unwrap_or_else(|| (String::new(), 0, false));
    Ok(TerminalOutputChunk {
        session_id,
        chunk,
        cursor: next_cursor,
        has_more,
        is_delta: true,
        at: now_timestamp(),
    })
}

#[tauri::command]
pub async fn resize_terminal_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    session_id: i64,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    terminal
        .resize_session(session_id, cols.max(2), rows.max(2))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn send_terminal_input_cmd(
    app: tauri::AppHandle,
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    session_id: i64,
    input: String,
) -> Result<(), String> {
    terminal
        .send_input(session_id, &input)
        .map_err(|e| e.to_string())?;
    db.insert_session_event(
        session_id,
        "input",
        None,
        Some(&serde_json::json!({ "input": input }).to_string()),
    )
    .await
    .map_err(|e| e.to_string())?;
    db.clear_session_needs_input(session_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(session) = db.get_managed_session(session_id).await {
        if let Some(agent_id) = session.agent_id {
            let _ = emit_agent_updated(&app, agent_id);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn send_terminal_data_cmd(
    app: tauri::AppHandle,
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    session_id: i64,
    data: String,
) -> Result<(), String> {
    terminal
        .send_input(session_id, &data)
        .map_err(|e| e.to_string())?;
    db.clear_session_needs_input(session_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(session) = db.get_managed_session(session_id).await {
        if let Some(agent_id) = session.agent_id {
            let _ = emit_agent_updated(&app, agent_id);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn attach_terminal_session_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    session_id: i64,
) -> Result<ManagedSession, String> {
    terminal
        .attach_session(db.inner(), session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn detach_terminal_session_cmd(
    db: tauri::State<'_, Db>,
    session_id: i64,
) -> Result<ManagedSession, String> {
    db.detach_terminal_session(session_id)
        .await
        .map_err(|e| e.to_string())
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
