use crate::{
    config::EchoConfig,
    db::Db,
    terminal::TerminalManager,
    voice::{VoiceManager, VoiceStatus},
};

#[tauri::command]
pub async fn start_voice_cmd(
    app: tauri::AppHandle,
    voice: tauri::State<'_, VoiceManager>,
    config: tauri::State<'_, EchoConfig>,
    db: tauri::State<'_, Db>,
    terminal: tauri::State<'_, TerminalManager>,
) -> Result<VoiceStatus, String> {
    voice
        .start(
            &app,
            config.inner(),
            db.inner().clone(),
            terminal.inner().clone(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_voice_cmd(
    app: tauri::AppHandle,
    voice: tauri::State<'_, VoiceManager>,
) -> Result<VoiceStatus, String> {
    voice.stop(&app).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn voice_status_cmd(
    voice: tauri::State<'_, VoiceManager>,
) -> Result<VoiceStatus, String> {
    Ok(voice.status())
}

#[tauri::command]
pub async fn process_voice_text_cmd(
    app: tauri::AppHandle,
    voice: tauri::State<'_, VoiceManager>,
    db: tauri::State<'_, Db>,
    terminal: tauri::State<'_, TerminalManager>,
    config: tauri::State<'_, EchoConfig>,
    text: String,
) -> Result<serde_json::Value, String> {
    voice
        .process_text(&app, db.inner(), terminal.inner(), config.inner(), text)
        .await
        .map_err(|e| e.to_string())
}
