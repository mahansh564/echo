use crate::commands::emit_agent_updated;
use crate::db::models::SessionAlert;
use crate::db::Db;

pub async fn list_session_alerts(
    db: &Db,
    agent_id: Option<i64>,
    unresolved_only: Option<bool>,
    limit: Option<i64>,
) -> anyhow::Result<Vec<SessionAlert>> {
    let unresolved = unresolved_only.unwrap_or(true);
    db.list_session_alerts(agent_id, unresolved, limit).await
}

pub async fn acknowledge_session_alert(db: &Db, alert_id: i64) -> anyhow::Result<SessionAlert> {
    db.acknowledge_session_alert(alert_id).await
}

pub async fn resolve_session_alert(db: &Db, alert_id: i64) -> anyhow::Result<SessionAlert> {
    db.resolve_session_alert(alert_id).await
}

#[tauri::command]
pub async fn list_session_alerts_cmd(
    db: tauri::State<'_, Db>,
    agent_id: Option<i64>,
    unresolved_only: Option<bool>,
    limit: Option<i64>,
) -> Result<Vec<SessionAlert>, String> {
    list_session_alerts(&db, agent_id, unresolved_only, limit)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn acknowledge_session_alert_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    alert_id: i64,
) -> Result<SessionAlert, String> {
    let alert = acknowledge_session_alert(&db, alert_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(agent_id) = alert.agent_id {
        let _ = emit_agent_updated(&app, agent_id);
    }
    Ok(alert)
}

#[tauri::command]
pub async fn resolve_session_alert_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    alert_id: i64,
) -> Result<SessionAlert, String> {
    let alert = resolve_session_alert(&db, alert_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(agent_id) = alert.agent_id {
        let _ = emit_agent_updated(&app, agent_id);
    }
    Ok(alert)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_session_alerts_returns_unresolved_default() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let agent = db
            .create_agent("Agent AlertCmd", Some("opencode"), None, None)
            .await
            .expect("agent");
        let session = db
            .create_managed_session(
                "opencode",
                "opencode",
                "[]",
                None,
                Some(agent.id),
                None,
                None,
            )
            .await
            .expect("session");

        let alert = db
            .create_session_alert(
                session.id,
                Some(agent.id),
                "warning",
                "input_prompt",
                "Please confirm",
                true,
            )
            .await
            .expect("alert");
        let _ = db.resolve_session_alert(alert.id).await.expect("resolved");

        let open_only = list_session_alerts(&db, Some(agent.id), None, Some(10))
            .await
            .expect("list unresolved");
        assert!(open_only.is_empty());

        let with_resolved = list_session_alerts(&db, Some(agent.id), Some(false), Some(10))
            .await
            .expect("list all");
        assert_eq!(with_resolved.len(), 1);
    }

    #[tokio::test]
    async fn acknowledge_and_resolve_alert_commands() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let agent = db
            .create_agent("Agent AlertFlow", Some("opencode"), None, None)
            .await
            .expect("agent");
        let session = db
            .create_managed_session(
                "opencode",
                "opencode",
                "[]",
                None,
                Some(agent.id),
                None,
                None,
            )
            .await
            .expect("session");
        let alert = db
            .create_session_alert(
                session.id,
                Some(agent.id),
                "warning",
                "input_prompt",
                "Needs input",
                true,
            )
            .await
            .expect("alert");

        let acked = acknowledge_session_alert(&db, alert.id)
            .await
            .expect("acknowledged");
        assert!(acked.acknowledged_at.is_some());

        let resolved = resolve_session_alert(&db, alert.id)
            .await
            .expect("resolved");
        assert!(resolved.resolved_at.is_some());
    }
}
