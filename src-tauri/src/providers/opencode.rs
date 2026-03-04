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

    fn parse_json_line(line: &str) -> Option<serde_json::Value> {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(trimmed).ok()
    }
}

impl ProviderAdapter for OpenCodeAdapter {
    fn provider_name(&self) -> &'static str {
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

    fn parse_structured_event(&self, chunk: &str) -> Result<Option<ProviderStructuredEvent>> {
        for line in chunk.lines() {
            let Some(value) = Self::parse_json_line(line) else {
                continue;
            };
            let event_type = value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            match event_type {
                "input_required" | "approval_required" | "tool_confirmation" => {
                    let reason = value
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("input_prompt")
                        .to_string();
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

                    return Ok(Some(ProviderStructuredEvent::InputRequired {
                        severity,
                        reason,
                        message,
                        requires_ack,
                    }));
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
                }
                _ => {}
            }
        }

        Ok(None)
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
