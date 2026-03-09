use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    commands,
    db::{
        models::{Agent, ManagedSession, SessionStatusSummary, StartSessionRequest},
        Db,
    },
    telemetry::Telemetry,
    terminal::TerminalManager,
    voice::{resolver, IntentCommand},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionPromptRequiredEvent {
    reason: String,
    source: String,
    action: Option<String>,
    message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentLocator {
    pub id: i64,
    pub name: String,
    pub display_order: i64,
}

pub async fn execute_command(
    app: &AppHandle,
    db: &Db,
    terminal: &TerminalManager,
    model_endpoint: &str,
    intent: &IntentCommand,
) -> Result<serde_json::Value> {
    match intent.action.as_str() {
        "status_overview" => {
            let agents = db.list_agents().await?;
            let sessions = db.list_managed_sessions(None, Some(200)).await?;
            let unresolved = db.list_unresolved_session_alerts(None, Some(200)).await?;
            let active_sessions = sessions
                .iter()
                .filter(|session| {
                    matches!(
                        session.status.as_str(),
                        "waking" | "active" | "stalled" | "needs_input"
                    )
                })
                .count();
            let answer = format!(
                "{} agents, {} active sessions, {} unresolved input requests.",
                agents.len(),
                active_sessions,
                unresolved.len()
            );
            Ok(serde_json::json!({
                "type": "status_overview",
                "answer": answer,
                "agents": agents.len(),
                "activeSessions": active_sessions,
                "unresolvedInputNeeded": unresolved.len(),
            }))
        }
        "list_input_needed" => {
            let alerts = db.list_unresolved_session_alerts(None, Some(200)).await?;
            let answer = if alerts.is_empty() {
                "No agents need input right now.".to_string()
            } else {
                format!("{} unresolved input requests need attention.", alerts.len())
            };
            Ok(serde_json::json!({
                "type": "input_needed_list",
                "answer": answer,
                "alerts": alerts,
            }))
        }
        "status_agent" => {
            let query = intent
                .payload
                .get("query")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let resolved = resolver::resolve_status_query(model_endpoint, query).await;

            let tasks = db.list_tasks().await?;
            let agents = db.list_agents().await?;
            let sessions = db.list_managed_sessions(None, Some(200)).await?;
            let locators = to_locators(&agents);

            let deterministic_agent_id = resolve_agent_id_from_payload(&locators, &intent.payload);
            let task = resolved.task_title_hint.as_ref().and_then(|hint| {
                let needle = hint.to_lowercase();
                tasks
                    .iter()
                    .find(|task| task.title.to_lowercase().contains(&needle))
                    .cloned()
            });
            let agent = deterministic_agent_id
                .and_then(|id| agents.iter().find(|agent| agent.id == id).cloned())
                .or_else(|| {
                    resolved.agent_index_hint.and_then(|index| {
                        resolve_agent_by_index(&locators, index).and_then(|locator| {
                            agents.iter().find(|agent| agent.id == locator.id).cloned()
                        })
                    })
                })
                .or_else(|| {
                    resolved.agent_name_hint.as_ref().and_then(|name_hint| {
                        let needle = name_hint.to_lowercase();
                        agents
                            .iter()
                            .find(|agent| agent.name.to_lowercase().contains(&needle))
                            .cloned()
                    })
                })
                .or_else(|| {
                    task.as_ref().and_then(|task_row| {
                        agents
                            .iter()
                            .find(|agent| agent.task_id == Some(task_row.id))
                            .cloned()
                    })
                });

            let Some(agent) = agent else {
                return emit_prompt_required(
                    app,
                    "ambiguous_target",
                    "status_agent",
                    "I could not determine which agent to target.",
                    "needs_agent_target",
                );
            };

            let session = find_session_for_agent(&agent, &sessions);
            let unresolved = db
                .list_unresolved_session_alerts(Some(agent.id), Some(200))
                .await?;
            let session_summary = session.as_ref().map(|s| SessionStatusSummary {
                session_id: s.id,
                status: s.status.clone(),
                agent_id: s.agent_id,
                task_id: s.task_id,
                last_heartbeat_at: s.last_heartbeat_at.clone(),
            });
            let task = task.or_else(|| {
                agent.task_id.and_then(|task_id| {
                    tasks
                        .iter()
                        .find(|task_row| task_row.id == task_id)
                        .cloned()
                })
            });

            let task_text = task
                .as_ref()
                .map(|task_row| task_row.title.clone())
                .unwrap_or_else(|| "no assigned task".to_string());
            let session_text = session
                .as_ref()
                .map(|managed| managed.status.clone())
                .unwrap_or_else(|| "no session".to_string());
            let answer = format!(
                "{} is linked to {} with session status {} and {} unresolved alerts.",
                agent.name,
                task_text,
                session_text,
                unresolved.len()
            );

            Ok(serde_json::json!({
                "type": "status_reply",
                "answer": answer,
                "resolved": resolved,
                "agent": agent,
                "task": task,
                "session": session,
                "sessionSummary": session_summary,
                "unresolvedAlerts": unresolved.len(),
            }))
        }
        "attach_agent" => {
            let agents = db.list_agents().await?;
            let agent = match resolve_agent_from_payload(&agents, &intent.payload) {
                AgentResolution::Resolved(agent) => agent,
                AgentResolution::Ambiguous(candidates) => {
                    if !is_confirmed(&intent.payload) {
                        let names = candidates
                            .iter()
                            .take(3)
                            .map(|agent| agent.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        return emit_confirmation_required(
                            app,
                            "attach_agent",
                            &format!(
                                "Ambiguous target. Multiple agents match: {}. Say confirm attach agent to proceed with the top match.",
                                names
                            ),
                        );
                    }
                    candidates
                        .first()
                        .cloned()
                        .ok_or_else(|| anyhow!("attach_agent requires a resolvable agent target"))?
                }
                AgentResolution::Missing => {
                    return emit_prompt_required(
                        app,
                        "ambiguous_target",
                        "attach_agent",
                        "I could not determine which agent to attach. Say attach agent and a number or exact name.",
                        "needs_agent_target",
                    );
                }
            };
            let sessions = db.list_managed_sessions(None, Some(200)).await?;
            let session = find_session_for_agent(&agent, &sessions);
            Ok(serde_json::json!({
                "type": "agent_attached",
                "agent": agent,
                "session": session,
            }))
        }
        "start_session" => {
            let agents = db.list_agents().await?;
            let agent = match resolve_agent_from_payload(&agents, &intent.payload) {
                AgentResolution::Resolved(agent) => Some(agent),
                AgentResolution::Ambiguous(candidates) => {
                    if !is_confirmed(&intent.payload) {
                        let names = candidates
                            .iter()
                            .take(3)
                            .map(|agent| agent.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        return emit_confirmation_required(
                            app,
                            "start_session",
                            &format!(
                                "Ambiguous target. Multiple agents match: {}. Say confirm start session to proceed with the top match.",
                                names
                            ),
                        );
                    }
                    candidates.first().cloned()
                }
                AgentResolution::Missing => None,
            };
            let command = intent
                .payload
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if command.is_none() {
                return emit_prompt_required(
                    app,
                    "missing_command",
                    "start_session",
                    "Command is required to start a session.",
                    "needs_command_prompt",
                );
            }
            let args = parse_args_payload(&intent.payload);
            let cwd = intent
                .payload
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string());

            let session = terminal
                .start_session(
                    app,
                    db.clone(),
                    StartSessionRequest {
                        command: command.unwrap_or("opencode").to_string(),
                        args,
                        cwd,
                        agent_id: agent.as_ref().map(|value| value.id),
                        task_id: agent.as_ref().and_then(|value| value.task_id),
                        provider: Some("opencode".to_string()),
                    },
                )
                .await?;

            Ok(serde_json::json!({
                "type": "session_started",
                "session": session,
                "agent": agent,
            }))
        }
        "stop_session" => {
            if !is_confirmed(&intent.payload) {
                return emit_confirmation_required(
                    app,
                    "stop_session",
                    "Stopping a session requires confirmation. Say confirm stop session for that agent.",
                );
            }
            let sessions = db.list_managed_sessions(None, Some(200)).await?;
            let agent = match resolve_agent_from_payload(&db.list_agents().await?, &intent.payload)
            {
                AgentResolution::Resolved(agent) => Some(agent),
                AgentResolution::Ambiguous(candidates) => candidates.first().cloned(),
                AgentResolution::Missing => None,
            };
            let session = find_session_from_payload(&sessions, &intent.payload, agent.as_ref())
                .ok_or_else(|| anyhow!("stop_session requires an active target session"))?;
            let telemetry = app.state::<Telemetry>();
            if let Err(err) = terminal.stop_session(db.clone(), session.id) {
                telemetry.record_session_stop_failed(
                    session.id,
                    "voice_router.stop_session",
                    &err.to_string(),
                );
                return Err(err);
            }
            telemetry.record_session_user_stop(
                session.id,
                session.agent_id.or(agent.as_ref().map(|value| value.id)),
                "voice_router.stop_session",
            );
            Ok(serde_json::json!({
                "type": "session_stopped",
                "sessionId": session.id,
                "agentId": agent.as_ref().map(|value| value.id),
            }))
        }
        "send_input" => {
            let input = intent
                .payload
                .get("input")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("send_input requires payload.input"))?;

            if looks_destructive_input(input) && !is_confirmed(&intent.payload) {
                return emit_confirmation_required(
                    app,
                    "send_input",
                    "This command looks destructive and needs confirmation. Say confirm tell agent to proceed.",
                );
            }

            let agents = db.list_agents().await?;
            let agent = match resolve_agent_from_payload(&agents, &intent.payload) {
                AgentResolution::Resolved(agent) => agent,
                AgentResolution::Ambiguous(candidates) => {
                    if !is_confirmed(&intent.payload) {
                        let names = candidates
                            .iter()
                            .take(3)
                            .map(|agent| agent.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        return emit_confirmation_required(
                            app,
                            "send_input",
                            &format!(
                                "Ambiguous target. Multiple agents match: {}. Say confirm tell agent to proceed with the top match.",
                                names
                            ),
                        );
                    }
                    candidates
                        .first()
                        .cloned()
                        .ok_or_else(|| anyhow!("send_input requires a resolvable agent target"))?
                }
                AgentResolution::Missing => {
                    return emit_prompt_required(
                        app,
                        "ambiguous_target",
                        "send_input",
                        "I could not determine which agent to send input to. Say agent number or exact name.",
                        "needs_agent_target",
                    );
                }
            };
            let sessions = db.list_managed_sessions(None, Some(200)).await?;
            let session = find_session_for_agent(&agent, &sessions)
                .ok_or_else(|| anyhow!("no active session found for target agent"))?;
            let payload = if input.ends_with('\n') {
                input.to_string()
            } else {
                format!("{}\n", input)
            };
            terminal.send_input(session.id, &payload)?;
            db.insert_session_event(
                session.id,
                "input",
                None,
                Some(&serde_json::json!({ "input": payload }).to_string()),
            )
            .await?;
            db.clear_session_needs_input(session.id).await?;

            Ok(serde_json::json!({
                "type": "input_sent",
                "agentId": agent.id,
                "sessionId": session.id,
                "input": input,
            }))
        }
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
            let agents = db.list_agents().await?;
            let agent = match resolve_agent_from_payload(&agents, &intent.payload) {
                AgentResolution::Resolved(agent) => Some(agent),
                AgentResolution::Ambiguous(candidates) => {
                    if !is_confirmed(&intent.payload) {
                        let names = candidates
                            .iter()
                            .take(3)
                            .map(|agent| agent.name.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        return emit_confirmation_required(
                            app,
                            "start_session",
                            &format!(
                                "Ambiguous target. Multiple agents match: {}. Say confirm start session to proceed with the top match.",
                                names
                            ),
                        );
                    }
                    candidates.first().cloned()
                }
                AgentResolution::Missing => None,
            };
            let command = intent
                .payload
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if command.is_none() {
                return emit_prompt_required(
                    app,
                    "missing_command",
                    "start_session",
                    "Command is required to start a session.",
                    "needs_command_prompt",
                );
            }
            let args = parse_args_payload(&intent.payload);
            let cwd = intent
                .payload
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string());
            let session = terminal
                .start_session(
                    app,
                    db.clone(),
                    StartSessionRequest {
                        command: command.unwrap_or("opencode").to_string(),
                        args,
                        cwd,
                        agent_id: agent.as_ref().map(|value| value.id),
                        task_id: agent.as_ref().and_then(|value| value.task_id),
                        provider: Some("opencode".to_string()),
                    },
                )
                .await?;
            Ok(serde_json::json!({
                "type": "session_started",
                "session": session,
                "agent": agent,
            }))
        }
        "query_agent_status" => {
            let query = intent
                .payload
                .get("query")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let resolved = resolver::resolve_status_query(model_endpoint, query).await;
            let tasks = db.list_tasks().await?;
            let agents = db.list_agents().await?;
            let sessions = db.list_managed_sessions(None, Some(200)).await?;
            let locators = to_locators(&agents);
            let task = resolved.task_title_hint.as_ref().and_then(|hint| {
                let needle = hint.to_lowercase();
                tasks
                    .iter()
                    .find(|task| task.title.to_lowercase().contains(&needle))
                    .cloned()
            });
            let agent = resolved
                .agent_index_hint
                .and_then(|index| resolve_agent_by_index(&locators, index))
                .and_then(|locator| agents.iter().find(|agent| agent.id == locator.id).cloned())
                .or_else(|| {
                    resolved.agent_name_hint.as_ref().and_then(|name_hint| {
                        let needle = name_hint.to_lowercase();
                        agents
                            .iter()
                            .find(|agent| agent.name.to_lowercase().contains(&needle))
                            .cloned()
                    })
                });
            let session = agent
                .as_ref()
                .and_then(|agent| find_session_for_agent(agent, &sessions));
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
                    .map(|task_row| task_row.title.clone())
                    .unwrap_or_else(|| "no assigned task".to_string());
                let session_text = session
                    .as_ref()
                    .map(|entry| entry.status.clone())
                    .unwrap_or_else(|| "no session".to_string());
                format!(
                    "{} is linked to {} with session status {}.",
                    agent.name, task_text, session_text
                )
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

fn parse_args_payload(payload: &Value) -> Vec<String> {
    payload
        .get("args")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|text| text.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn is_confirmed(payload: &Value) -> bool {
    payload
        .get("confirmed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn looks_destructive_input(input: &str) -> bool {
    let normalized = input.to_lowercase();
    let destructive_markers = [
        "rm -rf",
        "drop database",
        "delete",
        "destroy",
        "shutdown",
        "kill",
        "format",
    ];
    destructive_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

enum AgentResolution {
    Resolved(Agent),
    Ambiguous(Vec<Agent>),
    Missing,
}

fn find_session_from_payload(
    sessions: &[ManagedSession],
    payload: &Value,
    agent: Option<&Agent>,
) -> Option<ManagedSession> {
    if let Some(session_id) = payload.get("session_id").and_then(|value| value.as_i64()) {
        if let Some(session) = sessions
            .iter()
            .find(|entry| entry.id == session_id)
            .cloned()
        {
            return Some(session);
        }
    }
    agent.and_then(|value| find_session_for_agent(value, sessions))
}

fn find_session_for_agent(agent: &Agent, sessions: &[ManagedSession]) -> Option<ManagedSession> {
    if let Some(active_session_id) = agent.active_session_id {
        if let Some(session) = sessions
            .iter()
            .find(|entry| entry.id == active_session_id)
            .cloned()
        {
            return Some(session);
        }
    }
    sessions
        .iter()
        .find(|entry| {
            entry.agent_id == Some(agent.id)
                && matches!(
                    entry.status.as_str(),
                    "waking" | "active" | "stalled" | "needs_input"
                )
        })
        .cloned()
}

fn to_locators(agents: &[Agent]) -> Vec<AgentLocator> {
    agents
        .iter()
        .map(|agent| AgentLocator {
            id: agent.id,
            name: agent.name.clone(),
            display_order: agent.display_order,
        })
        .collect()
}

fn resolve_agent_from_payload(agents: &[Agent], payload: &Value) -> AgentResolution {
    let locators = to_locators(agents);
    if let Some(target_id) = resolve_agent_id_from_payload(&locators, payload) {
        if let Some(agent) = agents.iter().find(|agent| agent.id == target_id).cloned() {
            return AgentResolution::Resolved(agent);
        }
    }

    let name_hint = payload
        .get("agent_name_hint")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or_else(|| {
            payload
                .get("agent")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .or_else(|| {
            payload
                .get("query")
                .and_then(|value| value.as_str())
                .and_then(extract_agent_name_from_text)
        });
    let Some(name_hint) = name_hint else {
        return AgentResolution::Missing;
    };

    let needle = name_hint.trim().to_lowercase();
    if needle.is_empty() {
        return AgentResolution::Missing;
    }

    let mut matches = agents
        .iter()
        .filter(|agent| agent.name.to_lowercase().contains(&needle))
        .cloned()
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return AgentResolution::Missing;
    }
    if matches.len() == 1 {
        return AgentResolution::Resolved(matches.remove(0));
    }

    matches.sort_by_key(|agent| agent.display_order);
    AgentResolution::Ambiguous(matches)
}

pub(crate) fn resolve_agent_id_from_payload(
    agents: &[AgentLocator],
    payload: &Value,
) -> Option<i64> {
    if let Some(agent_id) = payload.get("agent_id").and_then(|value| value.as_i64()) {
        if agents.iter().any(|agent| agent.id == agent_id) {
            return Some(agent_id);
        }
    }

    if let Some(agent_index) = payload.get("agent_index").and_then(|value| value.as_i64()) {
        if let Some(agent) = resolve_agent_by_index(agents, agent_index) {
            return Some(agent.id);
        }
    }

    if let Some(alias) = payload
        .get("agent_alias")
        .and_then(|value| value.as_str())
        .and_then(nato_alias_to_index)
    {
        if let Some(agent) = resolve_agent_by_index(agents, alias as i64) {
            return Some(agent.id);
        }
    }

    if let Some(query) = payload.get("query").and_then(|value| value.as_str()) {
        if let Some(idx) = extract_agent_index_from_text(query) {
            if let Some(agent) = resolve_agent_by_index(agents, idx) {
                return Some(agent.id);
            }
        }
    }

    None
}

fn resolve_agent_by_index(agents: &[AgentLocator], index: i64) -> Option<AgentLocator> {
    if index <= 0 {
        return None;
    }
    let mut sorted = agents.to_vec();
    sorted.sort_by_key(|entry| entry.display_order);
    sorted.get((index - 1) as usize).cloned()
}

fn emit_prompt_required(
    app: &AppHandle,
    reason: &str,
    action: &str,
    message: &str,
    code: &str,
) -> Result<Value> {
    let _ = app.emit(
        "managed_session_prompt_required",
        SessionPromptRequiredEvent {
            reason: reason.to_string(),
            source: "voice".to_string(),
            action: Some(action.to_string()),
            message: Some(message.to_string()),
        },
    );
    Ok(serde_json::json!({
        "type": "prompt_required",
        "code": code,
        "message": message,
        "reason": reason,
        "source": "voice",
        "action": action,
    }))
}

fn emit_confirmation_required(app: &AppHandle, action: &str, message: &str) -> Result<Value> {
    let _ = app.emit(
        "managed_session_prompt_required",
        SessionPromptRequiredEvent {
            reason: "confirmation_required".to_string(),
            source: "voice".to_string(),
            action: Some(action.to_string()),
            message: Some(message.to_string()),
        },
    );
    Ok(serde_json::json!({
        "type": "confirmation_required",
        "code": "needs_confirmation",
        "message": message,
        "reason": "confirmation_required",
        "source": "voice",
        "action": action,
    }))
}

fn extract_agent_index_from_text(text: &str) -> Option<i64> {
    let normalized = text.to_lowercase();
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for (idx, token) in tokens.iter().enumerate() {
        if *token != "agent" {
            continue;
        }
        let next = tokens.get(idx + 1)?;
        let cleaned = next.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-');
        if let Ok(parsed) = cleaned.parse::<i64>() {
            if parsed > 0 {
                return Some(parsed);
            }
        }
        if let Some(nato_index) = nato_alias_to_index(cleaned) {
            return Some(nato_index as i64);
        }
    }
    None
}

fn extract_agent_name_from_text(text: &str) -> Option<String> {
    let normalized = text.to_lowercase();
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for (idx, token) in tokens.iter().enumerate() {
        if *token != "agent" {
            continue;
        }
        let next_raw = tokens.get(idx + 1)?;
        let cleaned = next_raw.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-');
        if cleaned.is_empty() {
            continue;
        }
        if cleaned
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .is_some()
        {
            return None;
        }
        if nato_alias_to_index(cleaned).is_some() {
            return None;
        }
        let start = normalized.find("agent")?;
        let tail = &text[start + "agent".len()..];
        let mut name = tail
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-');
        if name.is_empty() {
            name = cleaned;
        }
        return if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        };
    }
    None
}

fn nato_alias_to_index(alias: &str) -> Option<usize> {
    match alias {
        "alpha" => Some(1),
        "bravo" => Some(2),
        "charlie" => Some(3),
        "delta" => Some(4),
        "echo" => Some(5),
        "foxtrot" => Some(6),
        "golf" => Some(7),
        "hotel" => Some(8),
        "india" => Some(9),
        "juliet" => Some(10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_agents() -> Vec<Agent> {
        vec![
            Agent {
                id: 1,
                name: "Alpha Build".to_string(),
                state: "idle".to_string(),
                provider: "opencode".to_string(),
                display_order: 1,
                attention_state: "ok".to_string(),
                task_id: None,
                active_session_id: None,
                last_snippet: None,
                last_input_required_at: None,
                updated_at: "0".to_string(),
            },
            Agent {
                id: 2,
                name: "Alpha Test".to_string(),
                state: "idle".to_string(),
                provider: "opencode".to_string(),
                display_order: 2,
                attention_state: "ok".to_string(),
                task_id: None,
                active_session_id: None,
                last_snippet: None,
                last_input_required_at: None,
                updated_at: "0".to_string(),
            },
        ]
    }

    #[test]
    fn resolve_agent_from_payload_marks_ambiguous_name_hint() {
        let agents = sample_agents();
        let payload = serde_json::json!({ "agent_name_hint": "alpha" });
        match resolve_agent_from_payload(&agents, &payload) {
            AgentResolution::Ambiguous(matches) => assert_eq!(matches.len(), 2),
            _ => panic!("expected ambiguous match"),
        }
    }

    #[test]
    fn resolve_agent_from_payload_resolves_by_index() {
        let agents = sample_agents();
        let payload = serde_json::json!({ "agent_index": 2 });
        match resolve_agent_from_payload(&agents, &payload) {
            AgentResolution::Resolved(agent) => assert_eq!(agent.id, 2),
            _ => panic!("expected resolved match"),
        }
    }
}
