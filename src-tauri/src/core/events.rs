use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TaskUpdatedEvent {
    pub task_id: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentUpdatedEvent {
    pub agent_id: i64,
}
