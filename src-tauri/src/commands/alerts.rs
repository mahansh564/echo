use crate::commands::emit_agent_updated;
use crate::db::models::SessionAlert;
use crate::db::Db;
use serde::Serialize;
use tauri::Emitter;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionAlertResolvedEvent {
    alert_id: i64,
    session_id: i64,
    agent_id: Option<i64>,
    resolved_at: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionAlertSnoozedEvent {
    alert_id: i64,
    session_id: i64,
    agent_id: Option<i64>,
    snoozed_until: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionAlertEscalatedEvent {
    alert_id: i64,
    session_id: i64,
    agent_id: Option<i64>,
    severity: String,
    escalated_at: Option<String>,
    escalation_count: i64,
}

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

pub async fn snooze_session_alert(
    db: &Db,
    alert_id: i64,
    duration_minutes: Option<i64>,
) -> anyhow::Result<SessionAlert> {
    db.snooze_session_alert(alert_id, duration_minutes.unwrap_or(30))
        .await
}

pub async fn escalate_session_alert(db: &Db, alert_id: i64) -> anyhow::Result<SessionAlert> {
    db.escalate_session_alert(alert_id).await
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
    let _ = app.emit(
        "session_alert_resolved",
        SessionAlertResolvedEvent {
            alert_id: alert.id,
            session_id: alert.session_id,
            agent_id: alert.agent_id,
            resolved_at: alert.resolved_at.clone(),
        },
    );
    Ok(alert)
}

#[tauri::command]
pub async fn snooze_session_alert_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    alert_id: i64,
    duration_minutes: Option<i64>,
) -> Result<SessionAlert, String> {
    let alert = snooze_session_alert(&db, alert_id, duration_minutes)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(agent_id) = alert.agent_id {
        let _ = emit_agent_updated(&app, agent_id);
    }
    let _ = app.emit(
        "session_alert_snoozed",
        SessionAlertSnoozedEvent {
            alert_id: alert.id,
            session_id: alert.session_id,
            agent_id: alert.agent_id,
            snoozed_until: alert.snoozed_until.clone(),
        },
    );
    Ok(alert)
}

#[tauri::command]
pub async fn escalate_session_alert_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, Db>,
    alert_id: i64,
) -> Result<SessionAlert, String> {
    let alert = escalate_session_alert(&db, alert_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(agent_id) = alert.agent_id {
        let _ = emit_agent_updated(&app, agent_id);
    }
    let _ = app.emit(
        "session_alert_escalated",
        SessionAlertEscalatedEvent {
            alert_id: alert.id,
            session_id: alert.session_id,
            agent_id: alert.agent_id,
            severity: alert.severity.clone(),
            escalated_at: alert.escalated_at.clone(),
            escalation_count: alert.escalation_count,
        },
    );
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

    #[tokio::test]
    async fn snooze_and_escalate_alert_commands() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        let agent = db
            .create_agent("Agent Alert Escalate", Some("opencode"), None, None)
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
                "Needs attention",
                true,
            )
            .await
            .expect("alert");

        let snoozed = snooze_session_alert(&db, alert.id, Some(15))
            .await
            .expect("snoozed");
        assert!(snoozed.snoozed_until.is_some());

        let escalated = escalate_session_alert(&db, alert.id)
            .await
            .expect("escalated");
        assert_eq!(escalated.severity, "critical");
        assert_eq!(escalated.escalation_count, 1);
        assert!(escalated.escalated_at.is_some());
        assert!(escalated.snoozed_until.is_none());
    }
}
