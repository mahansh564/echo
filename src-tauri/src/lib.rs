use tauri::Manager;

use crate::{terminal::TerminalManager, voice::VoiceManager};

pub mod commands;
pub mod config;
pub mod core;
pub mod db;
pub mod linear;
pub mod providers;
pub mod terminal;
pub mod voice;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_dir)?;
            let db_path = app_dir.join("echo.db");
            let db_url = format!("sqlite://{}", db_path.display());
            let db = tauri::async_runtime::block_on(db::Db::connect(&db_url))?;
            app.manage(db);
            let terminal = TerminalManager::new();
            let db_state = app.state::<db::Db>().inner().clone();
            let _ = tauri::async_runtime::block_on(terminal.reconcile_orphan_sessions(&db_state));
            app.manage(terminal.clone());
            let config = config::load_config()?;
            let voice = VoiceManager::new();
            if config.voice_enabled {
                let db_state = app.state::<db::Db>().inner().clone();
                let _ = voice.start(app.handle(), &config, db_state, terminal.clone());
            }
            app.manage(config);
            app.manage(voice);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            commands::tasks::create_task_cmd,
            commands::tasks::update_task_cmd,
            commands::tasks::delete_task_cmd,
            commands::tasks::move_task_state_cmd,
            commands::tasks::list_tasks_cmd,
            commands::alerts::list_session_alerts_cmd,
            commands::alerts::acknowledge_session_alert_cmd,
            commands::alerts::resolve_session_alert_cmd,
            commands::alerts::snooze_session_alert_cmd,
            commands::alerts::escalate_session_alert_cmd,
            commands::agents::create_agent_cmd,
            commands::agents::assign_agent_to_task_cmd,
            commands::agents::list_agents_cmd,
            commands::agents::list_agent_rows_cmd,
            commands::terminal::start_agent_session_cmd,
            commands::terminal::stop_agent_session_cmd,
            commands::terminal::delete_managed_session_cmd,
            commands::terminal::list_managed_sessions_cmd,
            commands::terminal::list_session_events_cmd,
            commands::terminal::get_terminal_snippet_cmd,
            commands::terminal::get_terminal_output_cmd,
            commands::terminal::resize_terminal_cmd,
            commands::terminal::send_terminal_input_cmd,
            commands::terminal::send_terminal_data_cmd,
            commands::terminal::attach_terminal_session_cmd,
            commands::terminal::detach_terminal_session_cmd,
            commands::voice::start_voice_cmd,
            commands::voice::stop_voice_cmd,
            commands::voice::voice_status_cmd,
            commands::voice::process_voice_text_cmd,
            commands::voice::push_to_talk_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
