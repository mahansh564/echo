use anyhow::Result;

use crate::{
    db::models::{ManagedSession, StartSessionRequest},
    providers::{
        ProviderAdapter, ProviderSpawnSpec, ProviderStatusSnapshot, ProviderStructuredEvent,
    },
};

#[derive(Default)]
pub struct OpenCodeAdapter;

impl OpenCodeAdapter {
    pub fn new() -> Self {
        Self
    }

    fn strip_ansi_sequences(line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut chars = line.chars();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                match chars.next() {
                    Some('[') => {
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        let mut prev_esc = false;
                        for c in chars.by_ref() {
                            if c == '\u{7}' {
                                break;
                            }
                            if prev_esc && c == '\\' {
                                break;
                            }
                            prev_esc = c == '\u{1b}';
                        }
                    }
                    Some(_) | None => {}
                }
                continue;
            }
            out.push(ch);
        }
        out
    }

    fn normalize_line(line: &str) -> String {
        Self::strip_ansi_sequences(line)
            .replace('\r', "")
            .trim()
            .to_string()
    }

    fn parse_json_line(line: &str) -> Option<serde_json::Value> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return Some(parsed);
        }

        let start = trimmed.find('{')?;
        let end = trimmed.rfind('}')?;
        if end <= start {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]).ok()
    }

    fn canonical_alert_reason(event_type: &str, reason: Option<&str>) -> String {
        let normalized = reason.unwrap_or_default().trim().to_lowercase();
        let event = event_type.trim().to_lowercase();

        if normalized == "approval_needed"
            || normalized == "approval_required"
            || event == "approval_required"
        {
            return "approval_needed".to_string();
        }

        if normalized == "auth_needed"
            || normalized == "auth_required"
            || normalized == "authentication_required"
            || normalized == "login_required"
            || normalized == "reauth_required"
        {
            return "auth_needed".to_string();
        }

        if normalized == "tool_confirmation"
            || normalized == "confirmation_required"
            || event == "tool_confirmation"
        {
            return "tool_confirmation".to_string();
        }

        if normalized == "input_prompt"
            || normalized == "input_required"
            || normalized == "prompt_required"
            || normalized == "missing_input"
            || event == "input_required"
        {
            return "input_prompt".to_string();
        }

        if matches!(
            normalized.as_str(),
            "approval_needed" | "auth_needed" | "tool_confirmation" | "input_prompt" | "unknown"
        ) {
            return normalized;
        }

        "unknown".to_string()
    }

    fn detect_text_fallback(line: &str) -> Option<(String, String)> {
        let cleaned = Self::normalize_line(line);
        let normalized = cleaned.to_lowercase();
        if normalized.is_empty() {
            return None;
        }

        let reason = if normalized.contains("approval required")
            || normalized.contains("requires approval")
            || normalized.contains("need approval")
            || normalized.contains("approve this")
            || normalized.contains("approval needed")
        {
            "approval_needed"
        } else if normalized.contains("authentication required")
            || normalized.contains("auth required")
            || normalized.contains("login required")
            || normalized.contains("please log in")
            || normalized.contains("please login")
            || normalized.contains("sign in")
            || normalized.contains("token expired")
            || normalized.contains("credentials required")
        {
            "auth_needed"
        } else if normalized.contains("tool confirmation required")
            || normalized.contains("confirm tool execution")
            || normalized.contains("confirm this tool")
            || normalized.contains("allow tool execution")
            || normalized.contains("confirm to continue")
            || normalized.contains("are you sure")
            || normalized.contains("continue? (")
            || normalized.contains("(y/n)")
            || normalized.contains("[y/n]")
            || normalized.contains("[y/N]")
        {
            "tool_confirmation"
        } else if normalized.contains("input required")
            || normalized.contains("awaiting user input")
            || normalized.contains("please provide input")
            || normalized.contains("enter your response")
            || normalized.contains("please respond")
            || normalized.contains("choose an option")
            || normalized.contains("select an option")
            || normalized.contains("would you like to")
            || normalized.contains("what would you like")
            || normalized.contains("waiting for your input")
        {
            "input_prompt"
        } else {
            return None;
        };

        Some((reason.to_string(), cleaned))
    }
}

impl ProviderAdapter for OpenCodeAdapter {
    fn provider_name(&self) -> &str {
        "opencode"
    }

    fn spawn_session(&self, request: &StartSessionRequest) -> Result<ProviderSpawnSpec> {
        let command = if request.command.trim().is_empty() {
            "opencode".to_string()
        } else {
            request.command.trim().to_string()
        };
        Ok(ProviderSpawnSpec {
            command,
            args: request.args.clone(),
            cwd: request.cwd.clone(),
        })
    }

    fn parse_structured_event_line(&self, line: &str) -> Result<Option<ProviderStructuredEvent>> {
        let cleaned_line = Self::normalize_line(line);
        let Some(value) = Self::parse_json_line(&cleaned_line) else {
            if let Some((reason, message)) = Self::detect_text_fallback(&cleaned_line) {
                return Ok(Some(ProviderStructuredEvent::InputRequired {
                    severity: "warning".to_string(),
                    reason,
                    message,
                    requires_ack: true,
                }));
            }
            return Ok(None);
        };
        let event_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match event_type {
            "input_required" | "approval_required" | "tool_confirmation" => {
                let reason = Self::canonical_alert_reason(
                    event_type,
                    value.get("reason").and_then(|v| v.as_str()),
                );
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Session requires input")
                    .to_string();
                let severity = value
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .unwrap_or("warning")
                    .to_string();
                let requires_ack = value
                    .get("requires_ack")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                Ok(Some(ProviderStructuredEvent::InputRequired {
                    severity,
                    reason,
                    message,
                    requires_ack,
                }))
            }
            "status" => {
                if let Some(status) = value.get("status").and_then(|v| v.as_str()) {
                    let reason = value
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    return Ok(Some(ProviderStructuredEvent::SessionStatus {
                        status: status.to_string(),
                        reason,
                    }));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn build_status_snapshot(
        &self,
        session: &ManagedSession,
        _latest_output: Option<&str>,
    ) -> ProviderStatusSnapshot {
        ProviderStatusSnapshot {
            status: session.status.clone(),
            needs_input: session.needs_input,
            input_reason: session.input_reason.clone(),
        }
    }

    fn supports_terminal_attach(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderParseState;

    #[test]
    fn parse_structured_events_normalizes_alert_reasons() {
        let adapter = OpenCodeAdapter::new();
        let chunk = r#"{"type":"approval_required","message":"Need approval"}
{"type":"input_required","reason":"auth_required","message":"Login required"}"#;

        let events = adapter.parse_structured_events(chunk).expect("parse");
        assert_eq!(events.len(), 2);

        match &events[0] {
            ProviderStructuredEvent::InputRequired { reason, .. } => {
                assert_eq!(reason, "approval_needed")
            }
            _ => panic!("expected input required event"),
        }

        match &events[1] {
            ProviderStructuredEvent::InputRequired { reason, .. } => {
                assert_eq!(reason, "auth_needed")
            }
            _ => panic!("expected input required event"),
        }
    }

    #[test]
    fn parse_structured_events_keeps_multiple_events_per_chunk() {
        let adapter = OpenCodeAdapter::new();
        let chunk = r#"{"type":"status","status":"active"}
{"type":"status","status":"stalled","reason":"heartbeat_timeout"}"#;

        let events = adapter.parse_structured_events(chunk).expect("parse");
        assert_eq!(events.len(), 2);

        match &events[0] {
            ProviderStructuredEvent::SessionStatus { status, .. } => assert_eq!(status, "active"),
            _ => panic!("expected session status event"),
        }
        match &events[1] {
            ProviderStructuredEvent::SessionStatus { status, reason } => {
                assert_eq!(status, "stalled");
                assert_eq!(reason.as_deref(), Some("heartbeat_timeout"));
            }
            _ => panic!("expected session status event"),
        }
    }

    #[test]
    fn parse_stream_chunk_handles_chunk_split_json_lines() {
        let adapter = OpenCodeAdapter::new();
        let mut state = ProviderParseState::default();

        let first = adapter
            .parse_stream_chunk(
                r#"{"type":"input_required","reason":"auth_required","message":"Login"#,
                &mut state,
            )
            .expect("first parse");
        assert!(first.is_empty());

        let second = adapter
            .parse_stream_chunk(" required\"}\n", &mut state)
            .expect("second parse");
        assert_eq!(second.len(), 1);
        match &second[0] {
            ProviderStructuredEvent::InputRequired {
                reason, message, ..
            } => {
                assert_eq!(reason, "auth_needed");
                assert_eq!(message, "Login required");
            }
            _ => panic!("expected input required event"),
        }
    }

    #[test]
    fn parse_text_fallback_detects_high_confidence_prompts() {
        let adapter = OpenCodeAdapter::new();
        let chunk = "Authentication required: please login to continue\n";

        let events = adapter.parse_structured_events(chunk).expect("parse");
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderStructuredEvent::InputRequired { reason, .. } => {
                assert_eq!(reason, "auth_needed");
            }
            _ => panic!("expected input required event"),
        }
    }

    #[test]
    fn parse_text_fallback_ignores_non_prompt_lines() {
        let adapter = OpenCodeAdapter::new();
        let chunk = "Running unit tests...\nAll green.\n";
        let events = adapter.parse_structured_events(chunk).expect("parse");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_text_fallback_handles_ansi_wrapped_prompt() {
        let adapter = OpenCodeAdapter::new();
        let chunk = "\u{1b}[31mContinue? (y/N)\u{1b}[0m";
        let events = adapter.parse_structured_events(chunk).expect("parse");
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderStructuredEvent::InputRequired { reason, .. } => {
                assert_eq!(reason, "tool_confirmation");
            }
            _ => panic!("expected input required event"),
        }
    }

    #[test]
    fn parse_json_line_handles_embedded_json() {
        let adapter = OpenCodeAdapter::new();
        let chunk = "event: {\"type\":\"input_required\",\"message\":\"Need input\"}\n";
        let events = adapter.parse_structured_events(chunk).expect("parse");
        assert_eq!(events.len(), 1);
    }
}
