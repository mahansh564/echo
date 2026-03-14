use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ACTION_STATUS_OVERVIEW: &str = "status_overview";
pub const ACTION_STATUS_AGENT: &str = "status_agent";
pub const ACTION_START_SESSION: &str = "start_session";
pub const ACTION_STOP_SESSION: &str = "stop_session";
pub const ACTION_ATTACH_AGENT: &str = "attach_agent";
pub const ACTION_SEND_INPUT: &str = "send_input";
pub const ACTION_LIST_INPUT_NEEDED: &str = "list_input_needed";
pub const ACTION_CREATE_AGENT: &str = "create_agent";
pub const ACTION_UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntentCommand {
    pub action: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}

pub async fn parse_intent(model_endpoint: &str, input: &str) -> Result<IntentCommand> {
    let deterministic = parse_with_rules(input);
    if deterministic.action != ACTION_UNKNOWN {
        return Ok(deterministic);
    }

    if let Ok(intent) = parse_intent_with_llm(model_endpoint, input).await {
        let normalized = normalize_llm_intent(intent, input);
        if normalized.action != ACTION_UNKNOWN {
            return Ok(normalized);
        }
    }

    Ok(deterministic)
}

async fn parse_intent_with_llm(model_endpoint: &str, input: &str) -> Result<IntentCommand> {
    let endpoint = if model_endpoint.contains("/api/") {
        model_endpoint.to_string()
    } else {
        format!("{}/api/generate", model_endpoint.trim_end_matches('/'))
    };

    let prompt = format!(
        "Extract one command from this transcript. Return only JSON as {{\"action\": string, \"payload\": object}}.\nAllowed actions: status_overview, status_agent, start_session, stop_session, attach_agent, send_input, list_input_needed, create_agent, unknown.\nPayload contracts:\n- status_overview: {{\"confirmed\": boolean}}\n- status_agent: {{\"agent_index\": number|null, \"agent_name_hint\": string|null, \"query\": string, \"confirmed\": boolean}}\n- start_session: {{\"agent_index\": number|null, \"agent_name_hint\": string|null, \"command\": string|null, \"args\": string[], \"cwd\": string|null, \"confirmed\": boolean}}\n- stop_session: {{\"agent_index\": number|null, \"agent_name_hint\": string|null, \"confirmed\": boolean}}\n- attach_agent: {{\"agent_index\": number|null, \"agent_name_hint\": string|null, \"confirmed\": boolean}}\n- send_input: {{\"agent_index\": number|null, \"agent_name_hint\": string|null, \"input\": string, \"confirmed\": boolean}}\n- list_input_needed: {{\"confirmed\": boolean}}\n- create_agent: {{\"name\": string|null, \"confirmed\": boolean}}\nTranscript: {}",
        input
    );

    let response = reqwest::Client::new()
        .post(endpoint)
        .json(&serde_json::json!({
            "model": "llama3.2",
            "stream": false,
            "format": "json",
            "prompt": prompt,
        }))
        .send()
        .await?
        .error_for_status()?;

    let body: OllamaGenerateResponse = response.json().await?;
    let parsed: IntentCommand = serde_json::from_str(&body.response)?;
    Ok(parsed)
}

fn parse_with_rules(input: &str) -> IntentCommand {
    let trimmed = input.trim();
    let normalized = trimmed.to_lowercase();
    let confirmed = has_confirmation_prefix(&normalized);
    let normalized_without_confirmation = strip_confirmation_prefix(&normalized);
    let target = parse_agent_target(&normalized_without_confirmation);

    if normalized_without_confirmation.contains("needs input")
        || normalized_without_confirmation.contains("need input")
        || normalized_without_confirmation.contains("input needed")
        || normalized_without_confirmation.contains("pending input")
        || normalized_without_confirmation.contains("unresolved input")
        || normalized_without_confirmation.contains("open input")
        || normalized_without_confirmation.contains("unresolved alert")
        || normalized_without_confirmation.contains("open alert")
    {
        return IntentCommand {
            action: ACTION_LIST_INPUT_NEEDED.to_string(),
            payload: serde_json::json!({
                "confirmed": confirmed,
            }),
        };
    }

    if normalized_without_confirmation.contains("overview")
        || normalized_without_confirmation.contains("overall status")
        || normalized_without_confirmation.contains("status summary")
        || normalized_without_confirmation.contains("how many agents")
    {
        return IntentCommand {
            action: ACTION_STATUS_OVERVIEW.to_string(),
            payload: serde_json::json!({
                "confirmed": confirmed,
            }),
        };
    }

    if normalized_without_confirmation.contains("status")
        && (target.has_any_hint
            || normalized_without_confirmation.contains("agent ")
            || normalized_without_confirmation.contains("for "))
    {
        return IntentCommand {
            action: ACTION_STATUS_AGENT.to_string(),
            payload: serde_json::json!({
                "agent_index": target.agent_index,
                "agent_name_hint": target.agent_name_hint,
                "confirmed": confirmed,
                "query": trimmed,
            }),
        };
    }

    if normalized_without_confirmation.starts_with("status")
        || normalized_without_confirmation.contains("system status")
    {
        return IntentCommand {
            action: ACTION_STATUS_OVERVIEW.to_string(),
            payload: serde_json::json!({
                "confirmed": confirmed,
            }),
        };
    }

    if let Some(send_input_text) = parse_send_input_text(&normalized_without_confirmation, trimmed)
    {
        return IntentCommand {
            action: ACTION_SEND_INPUT.to_string(),
            payload: serde_json::json!({
                "agent_index": target.agent_index,
                "agent_name_hint": target.agent_name_hint,
                "input": send_input_text,
                "confirmed": confirmed,
            }),
        };
    }

    if normalized_without_confirmation.contains("attach")
        && (normalized_without_confirmation.contains("agent ") || target.has_any_hint)
    {
        return IntentCommand {
            action: ACTION_ATTACH_AGENT.to_string(),
            payload: serde_json::json!({
                "agent_index": target.agent_index,
                "agent_name_hint": target.agent_name_hint,
                "confirmed": confirmed,
            }),
        };
    }

    if normalized_without_confirmation.contains("start session")
        || normalized_without_confirmation.contains("open session")
        || normalized_without_confirmation.contains("resume session")
    {
        return IntentCommand {
            action: ACTION_START_SESSION.to_string(),
            payload: serde_json::json!({
                "agent_index": target.agent_index,
                "agent_name_hint": target.agent_name_hint,
                "command": parse_start_session_command(trimmed),
                "args": [],
                "cwd": serde_json::Value::Null,
                "confirmed": confirmed,
            }),
        };
    }

    if normalized_without_confirmation.contains("stop session")
        || normalized_without_confirmation.contains("end session")
        || normalized_without_confirmation.contains("terminate session")
    {
        return IntentCommand {
            action: ACTION_STOP_SESSION.to_string(),
            payload: serde_json::json!({
                "agent_index": target.agent_index,
                "agent_name_hint": target.agent_name_hint,
                "confirmed": confirmed,
            }),
        };
    }

    if is_create_agent_request(&normalized_without_confirmation) {
        return IntentCommand {
            action: ACTION_CREATE_AGENT.to_string(),
            payload: serde_json::json!({
                "name": parse_create_agent_name(trimmed),
                "confirmed": confirmed,
            }),
        };
    }

    IntentCommand {
        action: ACTION_UNKNOWN.to_string(),
        payload: serde_json::json!({ "raw": input }),
    }
}

fn normalize_llm_intent(intent: IntentCommand, input: &str) -> IntentCommand {
    let action = intent.action.trim().to_lowercase();
    match action.as_str() {
        ACTION_STATUS_OVERVIEW
        | ACTION_STATUS_AGENT
        | ACTION_START_SESSION
        | ACTION_STOP_SESSION
        | ACTION_ATTACH_AGENT
        | ACTION_SEND_INPUT
        | ACTION_LIST_INPUT_NEEDED
        | ACTION_CREATE_AGENT => normalize_payload_for_action(&action, intent.payload, input),
        "query_agent_status" => {
            let query = intent
                .payload
                .get("query")
                .and_then(|value| value.as_str())
                .unwrap_or(input);
            let target = parse_agent_target(&query.to_lowercase());
            normalize_payload_for_action(
                ACTION_STATUS_AGENT,
                serde_json::json!({
                    "agent_index": target.agent_index,
                    "agent_name_hint": target.agent_name_hint,
                    "query": query,
                    "confirmed": false,
                }),
                input,
            )
        }
        "start_opencode_session" => normalize_payload_for_action(
            ACTION_START_SESSION,
            serde_json::json!({
                "agent_index": intent.payload.get("agent_index").and_then(|v| v.as_i64()),
                "agent_name_hint": intent.payload.get("agent_name_hint").and_then(|v| v.as_str()),
                "command": intent.payload.get("command").and_then(|v| v.as_str()),
                "args": intent.payload.get("args").cloned().unwrap_or_else(|| serde_json::json!([])),
                "cwd": intent.payload.get("cwd").cloned().unwrap_or(Value::Null),
                "confirmed": false,
            }),
            input,
        ),
        _ => parse_with_rules(input),
    }
}

fn normalize_payload_for_action(action: &str, payload: Value, input: &str) -> IntentCommand {
    let input_target = parse_agent_target(&input.to_lowercase());
    let agent_index = payload
        .get("agent_index")
        .and_then(|value| value.as_i64())
        .or(input_target.agent_index);
    let agent_name_hint = payload
        .get("agent_name_hint")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or(input_target.agent_name_hint);
    let confirmed = payload
        .get("confirmed")
        .and_then(|value| value.as_bool())
        .unwrap_or_else(|| has_confirmation_prefix(&input.to_lowercase()));

    let normalized_payload = match action {
        ACTION_STATUS_OVERVIEW | ACTION_LIST_INPUT_NEEDED => {
            serde_json::json!({ "confirmed": confirmed })
        }
        ACTION_STATUS_AGENT => serde_json::json!({
            "agent_index": agent_index,
            "agent_name_hint": agent_name_hint,
            "query": payload.get("query").and_then(|value| value.as_str()).unwrap_or(input),
            "confirmed": confirmed,
        }),
        ACTION_START_SESSION => serde_json::json!({
            "agent_index": agent_index,
            "agent_name_hint": agent_name_hint,
            "command": payload.get("command").and_then(|value| value.as_str()),
            "args": payload.get("args").cloned().unwrap_or_else(|| serde_json::json!([])),
            "cwd": payload.get("cwd").cloned().unwrap_or(Value::Null),
            "confirmed": confirmed,
        }),
        ACTION_STOP_SESSION | ACTION_ATTACH_AGENT => serde_json::json!({
            "agent_index": agent_index,
            "agent_name_hint": agent_name_hint,
            "confirmed": confirmed,
        }),
        ACTION_SEND_INPUT => serde_json::json!({
            "agent_index": agent_index,
            "agent_name_hint": agent_name_hint,
            "input": payload.get("input").and_then(|value| value.as_str()).unwrap_or(""),
            "confirmed": confirmed,
        }),
        ACTION_CREATE_AGENT => serde_json::json!({
            "name": payload
                .get("name")
                .and_then(|value| value.as_str())
                .map(|value| value.trim())
                .filter(|value| !value.is_empty()),
            "confirmed": confirmed,
        }),
        _ => serde_json::json!({ "raw": input }),
    };

    IntentCommand {
        action: action.to_string(),
        payload: normalized_payload,
    }
}

#[derive(Default)]
struct ParsedAgentTarget {
    agent_index: Option<i64>,
    agent_name_hint: Option<String>,
    has_any_hint: bool,
}

fn parse_agent_target(normalized: &str) -> ParsedAgentTarget {
    let mut parsed = ParsedAgentTarget::default();
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for (idx, token) in tokens.iter().enumerate() {
        if *token != "agent" {
            continue;
        }
        let Some(next_raw) = tokens.get(idx + 1) else {
            continue;
        };
        let next = sanitize_token(next_raw);
        if next.is_empty() {
            continue;
        }
        if let Ok(index) = next.parse::<i64>() {
            if index > 0 {
                parsed.agent_index = Some(index);
                parsed.has_any_hint = true;
                return parsed;
            }
        }
        if let Some(index) = nato_to_index(next) {
            parsed.agent_index = Some(index as i64);
            parsed.agent_name_hint = Some(next.to_string());
            parsed.has_any_hint = true;
            return parsed;
        }
        parsed.agent_name_hint = Some(next.to_string());
        parsed.has_any_hint = true;
        return parsed;
    }
    parsed
}

fn nato_to_index(alias: &str) -> Option<usize> {
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

fn sanitize_token(token: &str) -> &str {
    token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
}

fn has_confirmation_prefix(normalized: &str) -> bool {
    normalized.starts_with("confirm ")
        || normalized.starts_with("yes ")
        || normalized == "confirm"
        || normalized == "yes"
}

fn strip_confirmation_prefix(normalized: &str) -> String {
    if let Some(value) = normalized.strip_prefix("confirm ") {
        return value.trim().to_string();
    }
    if let Some(value) = normalized.strip_prefix("yes ") {
        return value.trim().to_string();
    }
    normalized.to_string()
}

fn parse_send_input_text(normalized: &str, original: &str) -> Option<String> {
    let lower_original = original.to_lowercase();
    let patterns = [" tell ", " ask ", " send ", " message "];
    for pattern in patterns {
        if let Some(idx) = lower_original.find(pattern) {
            let tail = original[idx + pattern.len()..].trim();
            if let Some(stripped) = tail.strip_prefix("agent ") {
                if let Some(to_idx) = stripped.to_lowercase().find(" to ") {
                    let text = stripped[to_idx + 4..].trim();
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }

    if normalized.contains("agent ") && normalized.contains(" to ") {
        let marker = lower_original.find(" to ")?;
        let text = original[marker + 4..].trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

fn parse_start_session_command(original: &str) -> Value {
    let lower = original.to_lowercase();
    if let Some(idx) = lower.find(" with ") {
        let command = original[idx + 6..].trim();
        if !command.is_empty() {
            return Value::String(command.to_string());
        }
    }
    if let Some(idx) = lower.find(" running ") {
        let command = original[idx + 9..].trim();
        if !command.is_empty() {
            return Value::String(command.to_string());
        }
    }
    Value::Null
}

fn is_create_agent_request(normalized: &str) -> bool {
    let create_phrases = [
        "new agent",
        "create agent",
        "add agent",
        "spawn agent",
        "open a new chat",
        "open new chat",
        "new chat",
        "create chat",
        "start a new chat",
    ];
    create_phrases
        .iter()
        .any(|phrase| normalized.contains(phrase))
}

fn parse_create_agent_name(original: &str) -> Option<String> {
    let lower = original.to_lowercase();
    let markers = [" named ", " called ", " name "];

    for marker in markers {
        if let Some(idx) = lower.find(marker) {
            let name = original[idx + marker.len()..].trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fallback_rule_for_task_creation() {
        let intent = parse_intent("http://localhost:9", "status of agent 3")
            .await
            .expect("intent");
        assert_eq!(intent.action, "status_agent");
        assert_eq!(
            intent
                .payload
                .get("agent_index")
                .and_then(|value| value.as_i64()),
            Some(3)
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_alias_target() {
        let intent = parse_intent("http://localhost:9", "status of agent alpha")
            .await
            .expect("intent");
        assert_eq!(intent.action, "status_agent");
        assert_eq!(
            intent
                .payload
                .get("agent_index")
                .and_then(|value| value.as_i64()),
            Some(1)
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_send_input() {
        let intent = parse_intent("http://localhost:9", "tell agent 2 to run tests")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_SEND_INPUT);
        assert_eq!(
            intent
                .payload
                .get("agent_index")
                .and_then(|value| value.as_i64()),
            Some(2)
        );
        assert_eq!(
            intent.payload.get("input").and_then(|value| value.as_str()),
            Some("run tests")
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_status_overview() {
        let intent = parse_intent("http://localhost:9", "status overview")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_STATUS_OVERVIEW);
        assert_eq!(
            intent
                .payload
                .get("confirmed")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_start_session() {
        let intent = parse_intent(
            "http://localhost:9",
            "start session for agent 2 with opencode",
        )
        .await
        .expect("intent");
        assert_eq!(intent.action, ACTION_START_SESSION);
        assert_eq!(
            intent
                .payload
                .get("agent_index")
                .and_then(|value| value.as_i64()),
            Some(2)
        );
        assert_eq!(
            intent
                .payload
                .get("command")
                .and_then(|value| value.as_str()),
            Some("opencode")
        );
        assert!(intent.payload.get("args").is_some());
        assert!(intent.payload.get("cwd").is_some());
    }

    #[tokio::test]
    async fn fallback_rule_for_stop_session() {
        let intent = parse_intent("http://localhost:9", "confirm stop session for agent alpha")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_STOP_SESSION);
        assert_eq!(
            intent
                .payload
                .get("agent_index")
                .and_then(|value| value.as_i64()),
            Some(1)
        );
        assert_eq!(
            intent
                .payload
                .get("confirmed")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_attach_agent() {
        let intent = parse_intent("http://localhost:9", "attach agent bravo")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_ATTACH_AGENT);
        assert_eq!(
            intent
                .payload
                .get("agent_index")
                .and_then(|value| value.as_i64()),
            Some(2)
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_list_input_needed() {
        let intent = parse_intent("http://localhost:9", "which agents need input")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_LIST_INPUT_NEEDED);
        assert_eq!(
            intent
                .payload
                .get("confirmed")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_unresolved_inputs_query() {
        let intent = parse_intent("http://localhost:9", "show unresolved input alerts")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_LIST_INPUT_NEEDED);
    }

    #[tokio::test]
    async fn fallback_rule_for_open_new_chat() {
        let intent = parse_intent("http://localhost:9", "open a new chat")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_CREATE_AGENT);
        assert_eq!(
            intent.payload.get("name").and_then(|value| value.as_str()),
            None
        );
    }

    #[tokio::test]
    async fn fallback_rule_for_create_agent_named() {
        let intent = parse_intent("http://localhost:9", "create agent named Atlas")
            .await
            .expect("intent");
        assert_eq!(intent.action, ACTION_CREATE_AGENT);
        assert_eq!(
            intent.payload.get("name").and_then(|value| value.as_str()),
            Some("Atlas")
        );
    }
}
