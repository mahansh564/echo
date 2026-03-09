pub mod agents;
pub mod alerts;
pub mod tasks;
pub mod telemetry;
pub mod terminal;
pub mod voice;

use crate::core::events::{AgentUpdatedEvent, TaskUpdatedEvent};
use tauri::{AppHandle, Emitter};

pub fn emit_task_updated(app: &AppHandle, task_id: i64) -> Result<(), tauri::Error> {
    app.emit("task_updated", TaskUpdatedEvent { task_id })
}

pub fn emit_agent_updated(app: &AppHandle, agent_id: i64) -> Result<(), tauri::Error> {
    app.emit("agent_updated", AgentUpdatedEvent { agent_id })
}
