use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedQuery {
    pub task_title_hint: Option<String>,
    pub agent_name_hint: Option<String>,
    #[serde(default)]
    pub require_active_session: bool,
    pub raw_query: String,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}

pub async fn resolve_status_query(model_endpoint: &str, raw_query: &str) -> ResolvedQuery {
    if let Ok(resolved) = resolve_with_llm(model_endpoint, raw_query).await {
        return resolved;
    }

    let normalized = raw_query.to_lowercase();
    let mut task_title_hint = None;
    if let Some(idx) = normalized.find("working on") {
        let value = raw_query[idx + "working on".len()..].trim();
        if !value.is_empty() {
            task_title_hint = Some(value.to_string());
        }
    }

    ResolvedQuery {
        task_title_hint,
        agent_name_hint: None,
        require_active_session: normalized.contains("active") || normalized.contains("running"),
        raw_query: raw_query.to_string(),
    }
}

async fn resolve_with_llm(model_endpoint: &str, raw_query: &str) -> Result<ResolvedQuery> {
    let endpoint = if model_endpoint.contains("/api/") {
        model_endpoint.to_string()
    } else {
        format!("{}/api/generate", model_endpoint.trim_end_matches('/'))
    };

    let prompt = format!(
        "Resolve this status query into JSON: {{\"taskTitleHint\": string|null, \"agentNameHint\": string|null, \"requireActiveSession\": boolean, \"rawQuery\": string}}.\nReturn JSON only.\nQuery: {}",
        raw_query
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
    let mut parsed: ResolvedQuery = serde_json::from_str(&body.response)?;
    if parsed.raw_query.trim().is_empty() {
        parsed.raw_query = raw_query.to_string();
    }
    Ok(parsed)
}
