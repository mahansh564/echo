use std::time::Duration;

use serde::Deserialize;

const ENRICH_TIMEOUT_MS: u64 = 1_500;
const ENRICH_MODEL: &str = "llama3.2";
const ENRICH_MAX_CHARS: usize = 240;

#[derive(Debug, Clone)]
pub struct EnrichmentResult {
    pub cleaned: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}

#[derive(Debug, Deserialize)]
struct EnrichedPayload {
    cleaned: String,
}

pub async fn enrich_issue_message(model_endpoint: &str, raw_message: &str) -> EnrichmentResult {
    let normalized_raw = sanitize_display_text(raw_message, ENRICH_MAX_CHARS);
    if normalized_raw.is_empty() {
        return EnrichmentResult {
            cleaned: None,
            status: "failed".to_string(),
            error: Some("empty_message".to_string()),
        };
    }

    let endpoint = resolve_generate_endpoint(model_endpoint);
    let prompt = format!(
        "Rewrite this runtime issue for UI display. Keep it concise, factual, and action-oriented. Return JSON only as {{\\\"cleaned\\\":\\\"...\\\"}}. Max 28 words. Input: {}",
        normalized_raw
    );

    let request = reqwest::Client::builder()
        .timeout(Duration::from_millis(ENRICH_TIMEOUT_MS))
        .build();

    let client = match request {
        Ok(value) => value,
        Err(error) => {
            return EnrichmentResult {
                cleaned: None,
                status: "failed".to_string(),
                error: Some(format!("client_build: {error}")),
            }
        }
    };

    let response = client
        .post(endpoint)
        .json(&serde_json::json!({
            "model": ENRICH_MODEL,
            "stream": false,
            "format": "json",
            "prompt": prompt,
        }))
        .send()
        .await;

    let body = match response {
        Ok(value) => match value.error_for_status() {
            Ok(ok) => ok,
            Err(error) => {
                return EnrichmentResult {
                    cleaned: None,
                    status: "failed".to_string(),
                    error: Some(format!("http: {error}")),
                }
            }
        },
        Err(error) => {
            return EnrichmentResult {
                cleaned: None,
                status: "failed".to_string(),
                error: Some(format!("request: {error}")),
            }
        }
    };

    let decoded: OllamaGenerateResponse = match body.json().await {
        Ok(value) => value,
        Err(error) => {
            return EnrichmentResult {
                cleaned: None,
                status: "failed".to_string(),
                error: Some(format!("decode: {error}")),
            }
        }
    };

    let payload: EnrichedPayload = match serde_json::from_str(&decoded.response) {
        Ok(value) => value,
        Err(error) => {
            return EnrichmentResult {
                cleaned: None,
                status: "failed".to_string(),
                error: Some(format!("invalid_json: {error}")),
            }
        }
    };

    let cleaned = sanitize_display_text(&payload.cleaned, ENRICH_MAX_CHARS);
    if cleaned.is_empty() {
        return EnrichmentResult {
            cleaned: None,
            status: "failed".to_string(),
            error: Some("empty_output".to_string()),
        };
    }

    EnrichmentResult {
        cleaned: Some(cleaned),
        status: "success".to_string(),
        error: None,
    }
}

pub fn resolve_generate_endpoint(model_endpoint: &str) -> String {
    if model_endpoint.contains("/api/") {
        model_endpoint.to_string()
    } else {
        format!("{}/api/generate", model_endpoint.trim_end_matches('/'))
    }
}

pub fn sanitize_display_text(raw: &str, max_chars: usize) -> String {
    if raw.trim().is_empty() || max_chars == 0 {
        return String::new();
    }

    let stripped = strip_ansi_sequences(raw);
    let mut normalized = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' {
            normalized.push(' ');
            continue;
        }
        if ch.is_control() {
            continue;
        }
        normalized.push(ch);
    }

    let collapsed = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(collapsed.trim(), max_chars)
}

fn strip_ansi_sequences(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }

        let Some(next) = chars.next() else {
            break;
        };
        match next {
            '[' => {
                for seq_char in chars.by_ref() {
                    if ('@'..='~').contains(&seq_char) {
                        break;
                    }
                }
            }
            ']' => {
                let mut prev = '\0';
                for seq_char in chars.by_ref() {
                    if seq_char == '\u{7}' || (prev == '\u{1b}' && seq_char == '\\') {
                        break;
                    }
                    prev = seq_char;
                }
            }
            'P' | '_' | '^' => {
                let mut prev = '\0';
                for seq_char in chars.by_ref() {
                    if prev == '\u{1b}' && seq_char == '\\' {
                        break;
                    }
                    prev = seq_char;
                }
            }
            _ => {}
        }
    }
    out
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 || input.is_empty() {
        return String::new();
    }
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }

    let head = input
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{}…", head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_generate_endpoint_appends_path() {
        assert_eq!(
            resolve_generate_endpoint("http://localhost:11434"),
            "http://localhost:11434/api/generate"
        );
        assert_eq!(
            resolve_generate_endpoint("http://localhost:11434/api/generate"),
            "http://localhost:11434/api/generate"
        );
    }

    #[test]
    fn sanitize_display_text_strips_ansi_and_controls() {
        let value = sanitize_display_text("\u{1b}[31merror\u{1b}[0m\nline\0\tmore", 64);
        assert_eq!(value, "error line more");
    }

    #[test]
    fn sanitize_display_text_truncates() {
        let value = sanitize_display_text("1234567890", 5);
        assert_eq!(value, "1234…");
    }
}
