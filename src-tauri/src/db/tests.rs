use super::*;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Executor};
use std::str::FromStr;

async fn setup_test_db() -> Db {
    Db::connect("sqlite::memory:").await.expect("db init")
}

#[tokio::test]
async fn create_and_fetch_task() {
    let db = setup_test_db().await;
    let task = db.create_task("Refactor X", None).await.unwrap();
    let fetched = db.get_task(task.id).await.unwrap();
    assert_eq!(fetched.title, "Refactor X");
}

#[tokio::test]
async fn list_tasks_returns_created() {
    let db = setup_test_db().await;
    let _ = db.create_task("First task", None).await.unwrap();
    let _ = db.create_task("Second task", None).await.unwrap();
    let tasks = db.list_tasks().await.unwrap();
    assert!(tasks.len() >= 2);
}

#[tokio::test]
async fn list_agents_returns_created() {
    let db = setup_test_db().await;
    let created = db
        .create_agent("Agent One", Some("opencode"), None, None)
        .await
        .unwrap();
    assert_eq!(created.provider, "opencode");
    assert_eq!(created.attention_state, "ok");
    assert_eq!(created.display_order, created.id);
    let agents = db.list_agents().await.unwrap();
    assert!(agents.len() >= 1);
}

#[tokio::test]
async fn update_agent_snippet_persists() {
    let db = setup_test_db().await;
    let agent = db
        .create_agent("Agent Snip", Some("opencode"), None, None)
        .await
        .unwrap();
    db.update_agent_snippet(agent.id, "hello").await.unwrap();
    let agents = db.list_agents().await.unwrap();
    let found = agents.iter().find(|a| a.id == agent.id).unwrap();
    assert_eq!(found.last_snippet.as_deref(), Some("hello"));
}

#[tokio::test]
async fn managed_session_lifecycle_persists() {
    let db = setup_test_db().await;
    let session = db
        .create_managed_session("opencode", "opencode", "[]", None, None, None, None)
        .await
        .unwrap();
    assert_eq!(session.status, "waking");

    db.update_session_status(session.id, "active", None)
        .await
        .unwrap();
    db.update_session_heartbeat(session.id).await.unwrap();
    db.insert_session_event(session.id, "spawned", Some("ok"), None)
        .await
        .unwrap();

    let stored = db.get_managed_session(session.id).await.unwrap();
    assert_eq!(stored.status, "active");
    assert!(!stored.needs_input);
    assert_eq!(stored.transport, "pty");

    let events = db.list_session_events(session.id, Some(10)).await.unwrap();
    assert!(!events.is_empty());
}

#[tokio::test]
async fn mark_session_stalled_if_not_needs_input_sets_stalled_for_active_session() {
    let db = setup_test_db().await;
    let session = db
        .create_managed_session("opencode", "opencode", "[]", None, None, None, None)
        .await
        .unwrap();
    db.update_session_status(session.id, "active", None)
        .await
        .unwrap();

    let marked = db
        .mark_session_stalled_if_not_needs_input(session.id)
        .await
        .unwrap();
    assert!(marked);

    let stored = db.get_managed_session(session.id).await.unwrap();
    assert_eq!(stored.status, "stalled");
    assert!(!stored.needs_input);
}

#[tokio::test]
async fn mark_session_stalled_if_not_needs_input_skips_when_needs_input() {
    let db = setup_test_db().await;
    let session = db
        .create_managed_session("opencode", "opencode", "[]", None, None, None, None)
        .await
        .unwrap();
    db.mark_session_needs_input(session.id, "idle_no_output", "Waiting for your input")
        .await
        .unwrap();

    let marked = db
        .mark_session_stalled_if_not_needs_input(session.id)
        .await
        .unwrap();
    assert!(!marked);

    let stored = db.get_managed_session(session.id).await.unwrap();
    assert_eq!(stored.status, "needs_input");
    assert!(stored.needs_input);
    assert_eq!(stored.input_reason.as_deref(), Some("idle_no_output"));
}

#[tokio::test]
async fn delete_managed_session_clears_agent_link_and_cascades_rows() {
    let db = setup_test_db().await;
    let agent = db
        .create_agent("Agent Delete", Some("opencode"), None, None)
        .await
        .unwrap();
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
        .unwrap();
    db.insert_session_event(session.id, "spawned", Some("ok"), None)
        .await
        .unwrap();
    db.create_session_alert(
        session.id,
        Some(agent.id),
        "warning",
        "input_prompt",
        "requires input",
        true,
    )
    .await
    .unwrap();

    db.delete_managed_session(session.id).await.unwrap();

    assert!(db.get_managed_session(session.id).await.is_err());
    let events = db.list_session_events(session.id, Some(10)).await.unwrap();
    assert!(events.is_empty());
    let alerts = db
        .list_session_alerts(Some(agent.id), false, Some(10))
        .await
        .unwrap();
    assert!(alerts.is_empty());
    let agents = db.list_agents().await.unwrap();
    let stored_agent = agents.iter().find(|row| row.id == agent.id).unwrap();
    assert_eq!(stored_agent.active_session_id, None);
}

#[tokio::test]
async fn terminal_attach_detach_updates_attach_count() {
    let db = setup_test_db().await;
    let session = db
        .create_managed_session("opencode", "opencode", "[]", None, None, None, None)
        .await
        .unwrap();
    db.update_session_status(session.id, "active", None)
        .await
        .unwrap();

    let attached = db.attach_terminal_session(session.id).await.unwrap();
    assert_eq!(attached.attach_count, 1);

    let attached_again = db.attach_terminal_session(session.id).await.unwrap();
    assert_eq!(attached_again.attach_count, 2);

    let detached = db.detach_terminal_session(session.id).await.unwrap();
    assert_eq!(detached.attach_count, 1);

    let detached_again = db.detach_terminal_session(session.id).await.unwrap();
    assert_eq!(detached_again.attach_count, 0);

    let detached_floor = db.detach_terminal_session(session.id).await.unwrap();
    assert_eq!(detached_floor.attach_count, 0);
}

#[tokio::test]
async fn list_agent_rows_includes_runtime_session_summary() {
    let db = setup_test_db().await;
    let task = db
        .create_task("Investigate flaky test", None)
        .await
        .unwrap();
    let agent = db
        .create_agent(
            "Agent Runtime",
            Some("opencode"),
            Some("running"),
            Some(task.id),
        )
        .await
        .unwrap();
    let session = db
        .create_managed_session(
            "opencode",
            "opencode",
            "[]",
            None,
            Some(agent.id),
            Some(task.id),
            None,
        )
        .await
        .unwrap();
    db.update_session_status(session.id, "active", None)
        .await
        .unwrap();

    let rows = db.list_agent_rows(Some(10)).await.unwrap();
    let row = rows.iter().find(|row| row.agent_id == agent.id).unwrap();
    assert_eq!(row.agent_name, "Agent Runtime");
    assert_eq!(row.task_title.as_deref(), Some("Investigate flaky test"));
    assert_eq!(row.active_session_id, Some(session.id));
    assert_eq!(row.active_session_status.as_deref(), Some("active"));
}

#[tokio::test]
async fn session_alert_ack_and_resolve_flow() {
    let db = setup_test_db().await;
    let agent = db
        .create_agent("Agent Alerts", Some("opencode"), None, None)
        .await
        .unwrap();
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
        .unwrap();

    let alert = db
        .create_session_alert(
            session.id,
            Some(agent.id),
            "warning",
            "input_prompt",
            "Agent requested confirmation to continue",
            true,
        )
        .await
        .unwrap();
    assert!(alert.requires_ack);
    assert!(alert.acknowledged_at.is_none());
    assert!(alert.snoozed_until.is_none());
    assert_eq!(alert.escalation_count, 0);
    assert!(alert.resolved_at.is_none());

    let unresolved = db
        .list_unresolved_session_alerts(Some(agent.id), Some(10))
        .await
        .unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].id, alert.id);

    let acknowledged = db.acknowledge_session_alert(alert.id).await.unwrap();
    assert!(acknowledged.acknowledged_at.is_some());
    assert!(acknowledged.resolved_at.is_none());

    let resolved = db.resolve_session_alert(alert.id).await.unwrap();
    assert!(resolved.resolved_at.is_some());
    let latency_ms = db.alert_resolution_latency_ms(alert.id).await.unwrap();
    assert!(latency_ms.is_some());
    assert!(latency_ms.unwrap_or_default() >= 0);

    let unresolved_after = db
        .list_unresolved_session_alerts(Some(agent.id), Some(10))
        .await
        .unwrap();
    assert!(unresolved_after.is_empty());

    let agents = db.list_agents().await.unwrap();
    let stored_agent = agents.iter().find(|row| row.id == agent.id).unwrap();
    assert_eq!(stored_agent.attention_state, "ok");
}

#[tokio::test]
async fn create_session_alert_infers_agent_and_deduplicates_open_alerts() {
    let db = setup_test_db().await;
    let agent = db
        .create_agent("Agent Alert Link", Some("opencode"), None, None)
        .await
        .unwrap();
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
        .unwrap();

    let first = db
        .create_session_alert(
            session.id,
            None,
            "warning",
            "tool_confirmation",
            "Please confirm tool execution",
            true,
        )
        .await
        .unwrap();
    assert_eq!(first.agent_id, Some(agent.id));

    let second = db
        .create_session_alert(
            session.id,
            None,
            "warning",
            "tool_confirmation",
            "Please confirm tool execution",
            true,
        )
        .await
        .unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.agent_id, Some(agent.id));

    let unresolved = db
        .list_unresolved_session_alerts(Some(agent.id), Some(20))
        .await
        .unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].id, first.id);

    let events = db.list_session_events(session.id, Some(20)).await.unwrap();
    let persisted_events = events
        .iter()
        .filter(|event| event.event_type == "session_alert_upserted")
        .count();
    assert_eq!(persisted_events, 2);
}

#[tokio::test]
async fn session_alert_snooze_and_escalate_flow() {
    let db = setup_test_db().await;
    let agent = db
        .create_agent("Agent Alert Actions", Some("opencode"), None, None)
        .await
        .unwrap();
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
        .unwrap();

    let alert = db
        .create_session_alert(
            session.id,
            Some(agent.id),
            "warning",
            "approval_needed",
            "Approve deployment",
            true,
        )
        .await
        .unwrap();
    let with_alert = db.get_agent(agent.id).await.unwrap();
    assert_eq!(with_alert.attention_state, "needs_input");

    let snoozed = db.snooze_session_alert(alert.id, 30).await.unwrap();
    assert!(snoozed.snoozed_until.is_some());
    let unresolved_after_snooze = db
        .list_unresolved_session_alerts(Some(agent.id), Some(20))
        .await
        .unwrap();
    assert!(unresolved_after_snooze.is_empty());
    let after_snooze = db.get_agent(agent.id).await.unwrap();
    assert_eq!(after_snooze.attention_state, "ok");

    let escalated = db.escalate_session_alert(alert.id).await.unwrap();
    assert_eq!(escalated.severity, "critical");
    assert!(escalated.escalated_at.is_some());
    assert_eq!(escalated.escalation_count, 1);
    assert!(escalated.snoozed_until.is_none());
    let unresolved_after_escalate = db
        .list_unresolved_session_alerts(Some(agent.id), Some(20))
        .await
        .unwrap();
    assert_eq!(unresolved_after_escalate.len(), 1);
    let after_escalate = db.get_agent(agent.id).await.unwrap();
    assert_eq!(after_escalate.attention_state, "blocked");
}

#[tokio::test]
async fn agent_attention_state_promotes_from_alert_severity() {
    let db = setup_test_db().await;
    let agent = db
        .create_agent("Agent Attention", Some("opencode"), None, None)
        .await
        .unwrap();
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
        .unwrap();

    let warning_alert = db
        .create_session_alert(
            session.id,
            Some(agent.id),
            "warning",
            "input_prompt",
            "Please provide additional context",
            true,
        )
        .await
        .unwrap();
    let with_warning = db.get_agent(agent.id).await.unwrap();
    assert_eq!(with_warning.attention_state, "needs_input");

    let critical_alert = db
        .create_session_alert(
            session.id,
            Some(agent.id),
            "critical",
            "auth_needed",
            "Authentication token expired",
            true,
        )
        .await
        .unwrap();
    let with_critical = db.get_agent(agent.id).await.unwrap();
    assert_eq!(with_critical.attention_state, "blocked");

    let _ = db.resolve_session_alert(critical_alert.id).await.unwrap();
    let after_critical_resolve = db.get_agent(agent.id).await.unwrap();
    assert_eq!(after_critical_resolve.attention_state, "needs_input");

    let _ = db.resolve_session_alert(warning_alert.id).await.unwrap();
    let after_all_resolved = db.get_agent(agent.id).await.unwrap();
    assert_eq!(after_all_resolved.attention_state, "ok");
}

#[tokio::test]
async fn session_needs_input_promotes_attention_state_without_alerts() {
    let db = setup_test_db().await;
    let agent = db
        .create_agent("Agent Session Attention", Some("opencode"), None, None)
        .await
        .unwrap();
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
        .unwrap();

    db.mark_session_needs_input(
        session.id,
        "input_prompt",
        "Session requires operator confirmation",
    )
    .await
    .unwrap();
    let while_needed = db.get_agent(agent.id).await.unwrap();
    assert_eq!(while_needed.attention_state, "needs_input");

    db.clear_session_needs_input(session.id).await.unwrap();
    let cleared = db.get_agent(agent.id).await.unwrap();
    assert_eq!(cleared.attention_state, "ok");
}

#[tokio::test]
async fn connect_upgrades_legacy_schema_with_phase1_defaults() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("legacy.sqlite");
    let db_url = format!("sqlite://{}", db_path.display());

    let options = SqliteConnectOptions::from_str(&db_url)
        .expect("sqlite options")
        .create_if_missing(true);
    let mut conn = sqlx::SqliteConnection::connect_with(&options)
        .await
        .expect("legacy db connect");

    conn.execute(include_str!("../../migrations/0001_init.sql"))
        .await
        .expect("migrate 0001");
    conn.execute(include_str!("../../migrations/0002_session_lifecycle.sql"))
        .await
        .expect("migrate 0002");
    sqlx::query("INSERT INTO agents (name, state, task_id, last_snippet) VALUES (?, ?, ?, ?)")
        .bind("Legacy Agent")
        .bind("idle")
        .bind(Option::<i64>::None)
        .bind("legacy boot")
        .execute(&mut conn)
        .await
        .expect("insert legacy agent");
    sqlx::query(
        "INSERT INTO managed_sessions (provider, status, launch_command, launch_args_json, cwd, pid, agent_id, task_id, last_heartbeat_at, started_at, ended_at, failure_reason, metadata_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, NULL, NULL, NULL)",
    )
    .bind("opencode")
    .bind("active")
    .bind("opencode")
    .bind("[]")
    .bind(Option::<String>::None)
    .bind(Option::<i64>::None)
    .bind(1_i64)
    .bind(Option::<i64>::None)
    .execute(&mut conn)
    .await
    .expect("insert legacy session");

    drop(conn);

    let upgraded = Db::connect(&db_url).await.expect("upgrade connect");
    let agents = upgraded.list_agents().await.expect("list upgraded agents");
    let agent = agents
        .iter()
        .find(|agent| agent.name == "Legacy Agent")
        .expect("legacy agent upgraded");
    assert_eq!(agent.provider, "opencode");
    assert_eq!(agent.attention_state, "ok");
    assert_eq!(agent.display_order, agent.id);

    let sessions = upgraded
        .list_managed_sessions(None, Some(10))
        .await
        .expect("list upgraded sessions");
    let session = sessions
        .iter()
        .find(|session| session.agent_id == Some(agent.id))
        .expect("legacy session upgraded");
    assert!(!session.needs_input);
    assert_eq!(session.transport, "pty");
    assert!(session.last_activity_at.is_some());
}
