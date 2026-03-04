use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub state: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: i64,
    pub name: String,
    pub state: String,
    pub provider: String,
    pub display_order: i64,
    pub attention_state: String,
    pub task_id: Option<i64>,
    pub active_session_id: Option<i64>,
    pub last_snippet: Option<String>,
    pub last_input_required_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSession {
    pub id: i64,
    pub provider: String,
    pub status: String,
    pub launch_command: String,
    pub launch_args_json: String,
    pub cwd: Option<String>,
    pub pid: Option<i64>,
    pub agent_id: Option<i64>,
    pub task_id: Option<i64>,
    pub last_heartbeat_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub needs_input: bool,
    pub input_reason: Option<String>,
    pub last_activity_at: Option<String>,
    pub transport: String,
    pub attach_count: i64,
    pub failure_reason: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvent {
    pub id: i64,
    pub session_id: i64,
    pub event_type: String,
    pub message: Option<String>,
    pub payload_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAlert {
    pub id: i64,
    pub session_id: i64,
    pub agent_id: Option<i64>,
    pub severity: String,
    pub reason: String,
    pub message: String,
    pub requires_ack: bool,
    pub acknowledged_at: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub agent_id: i64,
    pub agent_name: String,
    pub agent_state: String,
    pub provider: String,
    pub display_order: i64,
    pub attention_state: String,
    pub task_id: Option<i64>,
    pub task_title: Option<String>,
    pub active_session_id: Option<i64>,
    pub active_session_status: Option<String>,
    pub active_session_needs_input: Option<bool>,
    pub active_session_input_reason: Option<String>,
    pub last_activity_at: Option<String>,
    pub last_snippet: Option<String>,
    pub unresolved_alert_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSessionRequest {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub agent_id: Option<i64>,
    pub task_id: Option<i64>,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusSummary {
    pub session_id: i64,
    pub status: String,
    pub agent_id: Option<i64>,
    pub task_id: Option<i64>,
    pub last_heartbeat_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WakeActionResult {
    SessionStarted {
        session: ManagedSession,
    },
    StatusReply {
        answer: String,
        session: Option<SessionStatusSummary>,
    },
    PromptRequired {
        code: String,
        message: String,
    },
}
