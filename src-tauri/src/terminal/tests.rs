use super::*;
use crate::db::Db;
use std::thread;
use std::time::Duration;

#[test]
fn pty_spawns() {
    let manager = TerminalManager::new();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let session = match manager.start_session_for_test(&shell, &["-lc", "echo hello"]) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("skipping pty test: {err}");
            return;
        }
    };
    thread::sleep(Duration::from_millis(200));
    let snippet = manager.last_snippet(session.id as i64).unwrap_or_default();
    assert!(snippet.contains("hello"));
}

#[tokio::test]
async fn reconcile_orphan_sessions_marks_open_rows_failed() {
    let db = Db::connect("sqlite::memory:").await.expect("db");
    let session = db
        .create_managed_session("opencode", "opencode", "[]", None, None, None, None)
        .await
        .expect("session");
    db.update_session_status(session.id, "active", None)
        .await
        .expect("active");

    let manager = TerminalManager::new();
    let reconciled = manager
        .reconcile_orphan_sessions(&db)
        .await
        .expect("reconcile");
    assert_eq!(reconciled, 1);

    let updated = db.get_managed_session(session.id).await.expect("updated");
    assert_eq!(updated.status, "failed");
}
