use anyhow::Result;
use serde::{Deserialize, Serialize};

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
    if let Ok(intent) = parse_intent_with_llm(model_endpoint, input).await {
        return Ok(intent);
    }

    Ok(parse_with_rules(input))
}

async fn parse_intent_with_llm(model_endpoint: &str, input: &str) -> Result<IntentCommand> {
    let endpoint = if model_endpoint.contains("/api/") {
        model_endpoint.to_string()
    } else {
        format!("{}/api/generate", model_endpoint.trim_end_matches('/'))
    };

    let prompt = format!(
        "Extract one command from this transcript. Return only JSON as {{\"action\": string, \"payload\": object}}.\nAllowed actions: create_task, create_agent, assign_agent, list_tasks, list_agents, start_opencode_session, query_agent_status, unknown.\nFor create_task payload: {{\"title\": string}}.\nFor create_agent payload: {{\"name\": string|null}}.\nFor assign_agent payload: {{\"agent_id\": number, \"task_id\": number|null}}.\nFor start_opencode_session payload: {{\"command\": string|null, \"args\": string[], \"cwd\": string|null, \"agent_id\": number|null, \"task_id\": number|null}}.\nFor query_agent_status payload: {{\"query\": string}}.\nTranscript: {}",
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
    let normalized = input.trim().to_lowercase();
    if normalized.starts_with("create task") || normalized.contains("new task") {
        return IntentCommand {
            action: "create_task".to_string(),
            payload: serde_json::json!({ "title": input.trim() }),
        };
    }

    if normalized.contains("new agent")
        || normalized.contains("create agent")
        || normalized.contains("open a new agent")
        || normalized.starts_with("open agent")
        || normalized.starts_with("spawn agent")
    {
        return IntentCommand {
            action: "create_agent".to_string(),
            payload: serde_json::json!({ "name": serde_json::Value::Null }),
        };
    }

    if normalized.contains("list tasks") {
        return IntentCommand {
            action: "list_tasks".to_string(),
            payload: serde_json::json!({}),
        };
    }

    if normalized.contains("list agents") {
        return IntentCommand {
            action: "list_agents".to_string(),
            payload: serde_json::json!({}),
        };
    }

    if normalized.contains("start")
        && (normalized.contains("opencode") || normalized.contains("session"))
    {
        return IntentCommand {
            action: "start_opencode_session".to_string(),
            payload: serde_json::json!({
                "command": serde_json::Value::Null,
                "args": [],
                "cwd": serde_json::Value::Null,
                "agent_id": serde_json::Value::Null,
                "task_id": serde_json::Value::Null,
            }),
        };
    }

    if normalized.contains("status") {
        return IntentCommand {
            action: "query_agent_status".to_string(),
            payload: serde_json::json!({ "query": input.trim() }),
        };
    }

    IntentCommand {
        action: "unknown".to_string(),
        payload: serde_json::json!({ "raw": input }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fallback_rule_for_task_creation() {
        let intent = parse_intent("http://localhost:9", "create task write docs")
            .await
            .expect("intent");
        assert_eq!(intent.action, "create_task");
    }

    #[tokio::test]
    async fn fallback_rule_for_agent_creation() {
        let intent = parse_intent("http://localhost:9", "Open a new agent.")
            .await
            .expect("intent");
        assert_eq!(intent.action, "create_agent");
    }
}
