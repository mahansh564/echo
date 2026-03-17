use crate::{config::AppMode, shell};

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAppModeResponse {
    pub requested_mode: AppMode,
    pub applied_mode: AppMode,
}

#[tauri::command]
pub async fn set_app_mode_cmd(
    app: tauri::AppHandle,
    mode: AppMode,
) -> Result<SetAppModeResponse, String> {
    let applied_mode = shell::set_and_apply_mode(&app, mode).map_err(|err| err.to_string())?;
    Ok(SetAppModeResponse {
        requested_mode: mode,
        applied_mode,
    })
}
