use crate::commands::emit_agent_updated;
use crate::db::{
    models::{Agent, AgentRow},
    Db,
};

pub async fn create_agent(
    db: &Db,
    name: String,
    provider: Option<String>,
    state: Option<String>,
    task_id: Option<i64>,
) -> anyhow::Result<Agent> {
    let agent = db
        .create_agent(&name, provider.as_deref(), state.as_deref(), task_id)
        .await?;
    Ok(agent)
}

pub async fn assign_agent_to_task(
    db: &Db,
    agent_id: i64,
    task_id: Option<i64>,
) -> anyhow::Result<Agent> {
    let agent = db.assign_agent_to_task(agent_id, task_id).await?;
    Ok(agent)
}

pub async fn list_agents(db: &Db) -> anyhow::Result<Vec<Agent>> {
    let agents = db.list_agents().await?;
    Ok(agents)
}

pub async fn list_agent_rows(db: &Db, limit: Option<i64>) -> anyhow::Result<Vec<AgentRow>> {
    let rows = db.list_agent_rows(limit).await?;
    Ok(rows)
}

#[tauri::command]
pub async fn create_agent_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    name: String,
    provider: Option<String>,
    state: Option<String>,
    task_id: Option<i64>,
) -> Result<Agent, String> {
    let agent = create_agent(&db, name, provider, state, task_id)
        .await
        .map_err(|e| e.to_string())?;
    let _ = emit_agent_updated(&app, agent.id);
    Ok(agent)
}

#[tauri::command]
pub async fn assign_agent_to_task_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    agent_id: i64,
    task_id: Option<i64>,
) -> Result<Agent, String> {
    let agent = assign_agent_to_task(&db, agent_id, task_id)
        .await
        .map_err(|e| e.to_string())?;
    let _ = emit_agent_updated(&app, agent.id);
    Ok(agent)
}

#[tauri::command]
pub async fn list_agents_cmd(db: tauri::State<'_, Db>) -> Result<Vec<Agent>, String> {
    list_agents(&db).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_agent_rows_cmd(
    db: tauri::State<'_, Db>,
    limit: Option<i64>,
) -> Result<Vec<AgentRow>, String> {
    list_agent_rows(&db, limit).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_agents_command() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let _ = create_agent(
            &db,
            "Agent A".to_string(),
            Some("opencode".to_string()),
            None,
            None,
        )
        .await
        .expect("agent");
        let agents = list_agents(&db).await.expect("list");
        assert!(agents.len() >= 1);
    }

    #[tokio::test]
    async fn list_agent_rows_command() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let task = crate::commands::tasks::create_task(&db, "Audit runtime".to_string(), None)
            .await
            .expect("task");
        let _ = create_agent(
            &db,
            "Agent Row".to_string(),
            Some("opencode".to_string()),
            Some("running".to_string()),
            Some(task.id),
        )
        .await
        .expect("agent");
        let rows = list_agent_rows(&db, Some(10)).await.expect("rows");
        assert!(!rows.is_empty());
    }
}
