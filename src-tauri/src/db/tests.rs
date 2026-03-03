use super::*;

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
    let _ = db.create_agent("Agent One", None, None).await.unwrap();
    let agents = db.list_agents().await.unwrap();
    assert!(agents.len() >= 1);
}

#[tokio::test]
async fn update_agent_snippet_persists() {
    let db = setup_test_db().await;
    let agent = db.create_agent("Agent Snip", None, None).await.unwrap();
    db.update_agent_snippet(agent.id, "hello").await.unwrap();
    let agents = db.list_agents().await.unwrap();
    let found = agents.iter().find(|a| a.id == agent.id).unwrap();
    assert_eq!(found.last_snippet.as_deref(), Some("hello"));
}

#[tokio::test]
async fn managed_session_lifecycle_persists() {
    let db = setup_test_db().await;
    let session = db
        .create_managed_session(
            "opencode",
            "opencode",
            "[]",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(session.status, "waking");

    db.update_session_status(session.id, "active", None).await.unwrap();
    db.update_session_heartbeat(session.id).await.unwrap();
    db.insert_session_event(session.id, "spawned", Some("ok"), None)
        .await
        .unwrap();

    let stored = db.get_managed_session(session.id).await.unwrap();
    assert_eq!(stored.status, "active");

    let events = db.list_session_events(session.id, Some(10)).await.unwrap();
    assert!(!events.is_empty());
}
