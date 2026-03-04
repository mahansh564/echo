use anyhow::{anyhow, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{
    commands,
    db::{models::SessionStatusSummary, models::StartSessionRequest, Db},
    terminal::TerminalManager,
    voice::{resolver, IntentCommand},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionPromptRequiredEvent {
    reason: String,
    source: String,
}

pub async fn execute_command(
    app: &AppHandle,
    db: &Db,
    terminal: &TerminalManager,
    model_endpoint: &str,
    intent: &IntentCommand,
) -> Result<serde_json::Value> {
    match intent.action.as_str() {
        "create_task" => {
            let title = intent
                .payload
                .get("title")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow!("create_task requires payload.title"))?;
            let task = commands::tasks::create_task(db, title.to_string(), None).await?;
            Ok(serde_json::to_value(task)?)
        }
        "create_agent" => {
            let name = intent
                .payload
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| "Agent".to_string());
            let agent =
                commands::agents::create_agent(db, name, Some("opencode".to_string()), None, None)
                    .await?;
            Ok(serde_json::to_value(agent)?)
        }
        "assign_agent" => {
            let agent_id = intent
                .payload
                .get("agent_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| anyhow!("assign_agent requires payload.agent_id"))?;
            let task_id = intent.payload.get("task_id").and_then(|v| v.as_i64());
            let agent = commands::agents::assign_agent_to_task(db, agent_id, task_id).await?;
            Ok(serde_json::to_value(agent)?)
        }
        "list_tasks" => {
            let tasks = commands::tasks::list_tasks(db).await?;
            Ok(serde_json::to_value(tasks)?)
        }
        "list_agents" => {
            let agents = commands::agents::list_agents(db).await?;
            Ok(serde_json::to_value(agents)?)
        }
        "start_opencode_session" => {
            let command = intent
                .payload
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if command.is_none() {
                let _ = app.emit(
                    "managed_session_prompt_required",
                    SessionPromptRequiredEvent {
                        reason: "missing_command".to_string(),
                        source: "voice".to_string(),
                    },
                );
                return Ok(serde_json::json!({
                    "type": "prompt_required",
                    "code": "needs_command_prompt",
                    "message": "Command is required to start opencode session",
                    "reason": "missing_command",
                    "source": "voice"
                }));
            }

            let args = intent
                .payload
                .get("args")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let cwd = intent
                .payload
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let agent_id = intent.payload.get("agent_id").and_then(|v| v.as_i64());
            let task_id = intent.payload.get("task_id").and_then(|v| v.as_i64());

            let session = terminal.start_session(
                app,
                db.clone(),
                StartSessionRequest {
                    command: command.unwrap_or("opencode").to_string(),
                    args,
                    cwd,
                    agent_id,
                    task_id,
                    provider: Some("opencode".to_string()),
                },
            )?;

            Ok(serde_json::json!({
                "type": "session_started",
                "session": session,
            }))
        }
        "query_agent_status" => {
            let query = intent
                .payload
                .get("query")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            let resolved = resolver::resolve_status_query(model_endpoint, query).await;

            let tasks = db.list_tasks().await?;
            let agents = db.list_agents().await?;
            let sessions = db.list_managed_sessions(None, Some(200)).await?;

            let task = resolved.task_title_hint.as_ref().and_then(|hint| {
                let needle = hint.to_lowercase();
                tasks
                    .iter()
                    .find(|task| task.title.to_lowercase().contains(&needle))
                    .cloned()
            });

            let agent = if let Some(name_hint) = resolved.agent_name_hint.as_ref() {
                let needle = name_hint.to_lowercase();
                agents
                    .iter()
                    .find(|agent| agent.name.to_lowercase().contains(&needle))
                    .cloned()
            } else if let Some(task_row) = task.as_ref() {
                agents
                    .iter()
                    .find(|agent| agent.task_id == Some(task_row.id))
                    .cloned()
            } else {
                agents.first().cloned()
            };

            let session = sessions
                .iter()
                .find(|session| {
                    let matches_agent = agent
                        .as_ref()
                        .is_some_and(|a| session.agent_id == Some(a.id));
                    let matches_task = task.as_ref().is_some_and(|t| session.task_id == Some(t.id));
                    matches_agent || matches_task
                })
                .or_else(|| sessions.first())
                .cloned();
            let session_summary = session.as_ref().map(|s| SessionStatusSummary {
                session_id: s.id,
                status: s.status.clone(),
                agent_id: s.agent_id,
                task_id: s.task_id,
                last_heartbeat_at: s.last_heartbeat_at.clone(),
            });

            let answer = if let Some(agent) = agent.as_ref() {
                let task_text = task
                    .as_ref()
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "no matching task".to_string());
                let session_text = session
                    .as_ref()
                    .map(|s| format!("{}", s.status))
                    .unwrap_or_else(|| "no session".to_string());
                format!(
                    "{} is linked to {} with session status {}.",
                    agent.name, task_text, session_text
                )
            } else if let Some(task) = task.as_ref() {
                format!("No agent is currently assigned to {}.", task.title)
            } else {
                "I could not find a matching agent or task for that status query.".to_string()
            };

            Ok(serde_json::json!({
                "type": "status_reply",
                "answer": answer,
                "resolved": resolved,
                "agent": agent,
                "task": task,
                "session": session,
                "sessionSummary": session_summary,
            }))
        }
        "unknown" => Err(anyhow!("intent not recognized")),
        other => Err(anyhow!("unsupported action: {}", other)),
    }
}
