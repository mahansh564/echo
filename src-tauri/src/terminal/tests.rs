use super::*;
use crate::db::Db;
use std::thread;
use std::time::{Duration, Instant};

fn shell_path() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

fn start_shell_script(manager: &TerminalManager, script: &str) -> Option<TerminalSession> {
    let shell = shell_path();
    match manager.start_session_for_test(&shell, &["-lc", script]) {
        Ok(session) => Some(session),
        Err(err) => {
            eprintln!("skipping shell script test: {err}");
            None
        }
    }
}

fn wait_for_output_contains(
    manager: &TerminalManager,
    session_id: i64,
    needle: &str,
    timeout: Duration,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if manager
            .session_output(session_id)
            .unwrap_or_default()
            .contains(needle)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn pty_spawns() {
    let manager = TerminalManager::new();
    let shell = shell_path();
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

#[test]
fn output_chunk_cursor_advances() {
    let manager = TerminalManager::new();
    let shell = shell_path();
    let session = match manager.start_session_for_test(&shell, &["-lc", "printf hello-world"]) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("skipping chunk test: {err}");
            return;
        }
    };
    thread::sleep(Duration::from_millis(200));
    let first = manager
        .session_output_chunk(session.id as i64, 0, 5)
        .expect("chunk");
    assert!(!first.0.is_empty());
    let second = manager
        .session_output_chunk(session.id as i64, first.1, 1024)
        .expect("chunk");
    assert!(second.1 >= first.1);
}

#[test]
fn resize_session_succeeds() {
    let manager = TerminalManager::new();
    let shell = shell_path();
    let session = match manager.start_session_for_test(&shell, &["-lc", "echo resize"]) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("skipping resize test: {err}");
            return;
        }
    };
    manager
        .resize_session(session.id as i64, 100, 32)
        .expect("resize");
}

#[test]
fn tui_alt_screen_sequences_are_preserved() {
    let manager = TerminalManager::new();
    let Some(session) = start_shell_script(&manager, "printf '\\033[?1049hALT-SCREEN\\033[?1049l'")
    else {
        return;
    };

    assert!(wait_for_output_contains(
        &manager,
        session.id as i64,
        "ALT-SCREEN",
        Duration::from_secs(2)
    ));

    let output = manager
        .session_output(session.id as i64)
        .unwrap_or_default();
    assert!(output.contains("\u{1b}[?1049h"));
    assert!(output.contains("\u{1b}[?1049l"));
}

#[test]
fn repl_like_workflow_accepts_incremental_input() {
    let manager = TerminalManager::new();
    let Some(session) = start_shell_script(
        &manager,
        "printf 'READY\\n'; while IFS= read -r line; do [ \"$line\" = \"exit\" ] && printf 'BYE\\n' && break; printf 'ECHO:%s\\n' \"$line\"; done",
    ) else {
        return;
    };

    let session_id = session.id as i64;
    assert!(wait_for_output_contains(
        &manager,
        session_id,
        "READY",
        Duration::from_secs(2)
    ));

    manager
        .send_input(session_id, "status\n")
        .expect("send input");
    assert!(wait_for_output_contains(
        &manager,
        session_id,
        "ECHO:status",
        Duration::from_secs(2)
    ));

    manager.send_input(session_id, "exit\n").expect("send exit");
    assert!(wait_for_output_contains(
        &manager,
        session_id,
        "BYE",
        Duration::from_secs(2)
    ));
}

#[test]
fn resize_emits_winch_for_tui_processes() {
    let manager = TerminalManager::new();
    let Some(session) = start_shell_script(
        &manager,
        "trap 'stty size; exit 0' WINCH; echo WAITING-FOR-WINCH; while :; do sleep 0.1; done",
    ) else {
        return;
    };

    let session_id = session.id as i64;
    assert!(wait_for_output_contains(
        &manager,
        session_id,
        "WAITING-FOR-WINCH",
        Duration::from_secs(2)
    ));

    manager
        .resize_session(session_id, 120, 40)
        .expect("resize should send WINCH");
    assert!(wait_for_output_contains(
        &manager,
        session_id,
        "40 120",
        Duration::from_secs(2)
    ));
}

#[test]
fn test_runner_like_output_is_retrievable_in_small_chunks() {
    let manager = TerminalManager::new();
    let Some(session) = start_shell_script(
        &manager,
        "for i in 1 2 3 4 5; do printf 'test_%s ... ok\\n' \"$i\"; sleep 0.03; done; printf 'summary: 5 passed\\n'",
    ) else {
        return;
    };

    let session_id = session.id as i64;
    assert!(wait_for_output_contains(
        &manager,
        session_id,
        "summary: 5 passed",
        Duration::from_secs(2)
    ));

    let mut cursor = 0usize;
    let mut reconstructed = String::new();
    for _ in 0..64 {
        let Some((chunk, next_cursor, has_more)) =
            manager.session_output_chunk(session_id, cursor, 16)
        else {
            break;
        };
        if !chunk.is_empty() {
            reconstructed.push_str(&chunk);
        }
        cursor = next_cursor;
        if !has_more {
            break;
        }
    }

    assert!(reconstructed.contains("test_1 ... ok"));
    assert!(reconstructed.contains("test_5 ... ok"));
    assert!(reconstructed.contains("summary: 5 passed"));
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

#[tokio::test]
async fn stop_session_is_idempotent_for_db_open_rows_without_runtime() {
    let db = Db::connect("sqlite::memory:").await.expect("db");
    let session = db
        .create_managed_session("opencode", "opencode", "[]", None, None, None, None)
        .await
        .expect("session");
    db.update_session_status(session.id, "active", None)
        .await
        .expect("active");

    let manager = TerminalManager::new();
    manager
        .stop_session(db.clone(), session.id)
        .expect("stop should be idempotent");

    let start = Instant::now();
    loop {
        let updated = db
            .get_managed_session(session.id)
            .await
            .expect("session row");
        if updated.status == "ended" {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "timed out waiting for session to end"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn attach_session_rejects_when_runtime_is_missing() {
    let db = Db::connect("sqlite::memory:").await.expect("db");
    let session = db
        .create_managed_session("opencode", "opencode", "[]", None, None, None, None)
        .await
        .expect("session");
    db.update_session_status(session.id, "active", None)
        .await
        .expect("active");

    let manager = TerminalManager::new();
    let err = manager
        .attach_session(&db, session.id)
        .await
        .expect_err("attach should require an active runtime handle");
    assert!(err.to_string().contains("runtime is not available"));
}

#[test]
fn session_output_buffer_trims_to_limit_and_advances_base_cursor() {
    let mut buffer = SessionOutputBuffer::with_limit(8);
    assert_eq!(buffer.append("abcd"), 4);
    assert_eq!(buffer.append("efgh"), 8);
    assert_eq!(buffer.append("ijkl"), 12);

    assert_eq!(buffer.snapshot(), "efghijkl");
    let (chunk, next, has_more) = buffer.chunk(0, 8);
    assert_eq!(chunk, "efghijkl");
    assert_eq!(next, 12);
    assert!(!has_more);
}

#[test]
fn session_output_buffer_chunk_handles_cursor_before_trimmed_window() {
    let mut buffer = SessionOutputBuffer::with_limit(10);
    let _ = buffer.append("0123456789");
    let _ = buffer.append("abcdef");

    let (chunk, next, has_more) = buffer.chunk(2, 4);
    assert_eq!(chunk, "6789");
    assert_eq!(next, 10);
    assert!(has_more);
}
