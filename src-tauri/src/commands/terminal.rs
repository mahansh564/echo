use crate::db::models::{ManagedSession, SessionEvent, StartSessionRequest};
use crate::db::Db;
use crate::terminal::{TerminalManager, TerminalSession};

#[tauri::command]
pub async fn start_managed_session_cmd(
    app: tauri::AppHandle,
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    request: StartSessionRequest,
) -> Result<ManagedSession, String> {
    terminal
        .start_session(&app, db.inner().clone(), request)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_managed_session_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    session_id: i64,
) -> Result<(), String> {
    terminal
        .stop_session(db.inner().clone(), session_id)
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

// Deprecated compatibility command. Prefer `start_managed_session_cmd`.
#[tauri::command]
pub async fn start_terminal_session_cmd(
    app: tauri::AppHandle,
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    agent_id: i64,
) -> Result<TerminalSession, String> {
    terminal
        .start_session_legacy(&app, db.inner().clone(), agent_id)
        .map_err(|e| e.to_string())
}

// Deprecated compatibility command. Prefer `stop_managed_session_cmd`.
#[tauri::command]
pub async fn stop_terminal_session_cmd(
    terminal: tauri::State<'_, TerminalManager>,
    db: tauri::State<'_, Db>,
    session_id: u64,
) -> Result<(), String> {
    terminal
        .stop_session_legacy(db.inner().clone(), session_id)
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
) -> Result<String, String> {
    Ok(terminal.session_output(session_id).unwrap_or_default())
}

#[tauri::command]
pub async fn send_terminal_input_cmd(
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
    Ok(())
}
