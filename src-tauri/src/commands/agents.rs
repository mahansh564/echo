use crate::commands::emit_agent_updated;
use crate::db::{models::Agent, Db};

pub async fn create_agent(
    db: &Db,
    name: String,
    state: Option<String>,
    task_id: Option<i64>,
) -> anyhow::Result<Agent> {
    let agent = db
        .create_agent(&name, state.as_deref(), task_id)
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

#[tauri::command]
pub async fn create_agent_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    name: String,
    state: Option<String>,
    task_id: Option<i64>,
) -> Result<Agent, String> {
    let agent = create_agent(&db, name, state, task_id)
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


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_agents_command() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let _ = create_agent(&db, "Agent A".to_string(), None, None)
            .await
            .expect("agent");
        let agents = list_agents(&db).await.expect("list");
        assert!(agents.len() >= 1);
    }
}
