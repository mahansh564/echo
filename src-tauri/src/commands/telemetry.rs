use crate::telemetry::{Telemetry, TelemetrySnapshot};

#[tauri::command]
pub fn telemetry_snapshot_cmd(telemetry: tauri::State<'_, Telemetry>) -> TelemetrySnapshot {
    telemetry.snapshot()
}
