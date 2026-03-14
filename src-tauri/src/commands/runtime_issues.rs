use crate::config::EchoConfig;
use crate::db::models::RuntimeIssue;
use crate::db::Db;
use crate::issue_enrichment::enrich_issue_message;

const RUNTIME_ISSUE_DISMISS_TTL_MS: i64 = 120_000;

#[tauri::command]
pub async fn report_runtime_issue_cmd(
    db: tauri::State<'_, Db>,
    config: tauri::State<'_, EchoConfig>,
    kind: String,
    source: String,
    message: String,
) -> Result<RuntimeIssue, String> {
    let enrichment = enrich_issue_message(&config.model_endpoint, &message).await;
    db.report_runtime_issue(
        kind.trim(),
        source.trim(),
        message.trim(),
        enrichment.cleaned.as_deref(),
        &enrichment.status,
        enrichment.error.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_runtime_issues_cmd(
    db: tauri::State<'_, Db>,
    limit: Option<i64>,
) -> Result<Vec<RuntimeIssue>, String> {
    db.list_visible_runtime_issues(limit)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn dismiss_runtime_issue_cmd(
    db: tauri::State<'_, Db>,
    kind: String,
) -> Result<RuntimeIssue, String> {
    db.dismiss_runtime_issue(kind.trim(), RUNTIME_ISSUE_DISMISS_TTL_MS)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_runtime_issue_cmd(db: tauri::State<'_, Db>, kind: String) -> Result<(), String> {
    db.clear_runtime_issue(kind.trim())
        .await
        .map_err(|e| e.to_string())
}
