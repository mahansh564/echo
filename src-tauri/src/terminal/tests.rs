use super::*;
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
