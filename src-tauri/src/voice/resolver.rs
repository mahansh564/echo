use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedQuery {
    pub task_title_hint: Option<String>,
    pub agent_index_hint: Option<i64>,
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
    let deterministic = resolve_deterministic(raw_query);
    if deterministic.task_title_hint.is_some()
        || deterministic.agent_index_hint.is_some()
        || deterministic.agent_name_hint.is_some()
    {
        return deterministic;
    }

    if let Ok(llm_resolved) = resolve_with_llm(model_endpoint, raw_query).await {
        return merge_resolved(deterministic, llm_resolved);
    }

    deterministic
}

fn resolve_deterministic(raw_query: &str) -> ResolvedQuery {
    let normalized = raw_query.to_lowercase();
    let mut task_title_hint = None;
    if let Some(idx) = normalized.find("working on") {
        let value = raw_query[idx + "working on".len()..].trim();
        if !value.is_empty() {
            task_title_hint = Some(value.to_string());
        }
    }

    let (agent_index_hint, agent_name_hint) = extract_agent_hint_from_query(raw_query);

    ResolvedQuery {
        task_title_hint,
        agent_index_hint,
        agent_name_hint,
        require_active_session: normalized.contains("active") || normalized.contains("running"),
        raw_query: raw_query.to_string(),
    }
}

fn merge_resolved(deterministic: ResolvedQuery, llm: ResolvedQuery) -> ResolvedQuery {
    ResolvedQuery {
        task_title_hint: deterministic.task_title_hint.or(llm.task_title_hint),
        agent_index_hint: deterministic.agent_index_hint.or(llm.agent_index_hint),
        agent_name_hint: deterministic.agent_name_hint.or(llm.agent_name_hint),
        require_active_session: deterministic.require_active_session || llm.require_active_session,
        raw_query: if deterministic.raw_query.trim().is_empty() {
            llm.raw_query
        } else {
            deterministic.raw_query
        },
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

fn extract_agent_hint_from_query(raw_query: &str) -> (Option<i64>, Option<String>) {
    let normalized = raw_query.to_lowercase();
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
                return (Some(index), None);
            }
        }

        if let Some(index) = nato_alias_to_index(next) {
            return (Some(index as i64), Some(next.to_string()));
        }

        return (None, Some(next.to_string()));
    }
    (None, None)
}

fn sanitize_token(token: &str) -> &str {
    token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
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

    #[test]
    fn deterministic_resolver_extracts_agent_index() {
        let resolved = resolve_deterministic("status of agent 3");
        assert_eq!(resolved.agent_index_hint, Some(3));
        assert_eq!(resolved.agent_name_hint, None);
    }

    #[test]
    fn deterministic_resolver_extracts_alias() {
        let resolved = resolve_deterministic("status of agent alpha");
        assert_eq!(resolved.agent_index_hint, Some(1));
        assert_eq!(resolved.agent_name_hint.as_deref(), Some("alpha"));
    }

    #[test]
    fn deterministic_resolver_extracts_agent_name() {
        let resolved = resolve_deterministic("status of agent mason");
        assert_eq!(resolved.agent_index_hint, None);
        assert_eq!(resolved.agent_name_hint.as_deref(), Some("mason"));
    }
}
