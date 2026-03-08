use anyhow::Result;

use crate::db::models::{ManagedSession, StartSessionRequest};

pub mod generic;
pub mod opencode;

#[derive(Debug, Clone)]
pub struct ProviderSpawnSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderStatusSnapshot {
    pub status: String,
    pub needs_input: bool,
    pub input_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderParseState {
    pub pending_line: String,
}

#[derive(Debug, Clone)]
pub enum ProviderStructuredEvent {
    InputRequired {
        severity: String,
        reason: String,
        message: String,
        requires_ack: bool,
    },
    SessionStatus {
        status: String,
        reason: Option<String>,
    },
}

pub trait ProviderAdapter: Send + Sync {
    fn provider_name(&self) -> &str;

    fn spawn_session(&self, request: &StartSessionRequest) -> Result<ProviderSpawnSpec>;

    fn parse_structured_event_line(&self, _line: &str) -> Result<Option<ProviderStructuredEvent>> {
        Ok(None)
    }

    fn parse_structured_events(&self, chunk: &str) -> Result<Vec<ProviderStructuredEvent>> {
        let mut events = Vec::new();
        for raw_line in chunk.lines() {
            let line = raw_line.trim_end_matches('\r');
            if let Some(event) = self.parse_structured_event_line(line)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    fn parse_stream_chunk(
        &self,
        chunk: &str,
        state: &mut ProviderParseState,
    ) -> Result<Vec<ProviderStructuredEvent>> {
        state.pending_line.push_str(chunk);
        let mut events = Vec::new();

        while let Some(newline_idx) = state.pending_line.find('\n') {
            let mut line = state.pending_line[..newline_idx].to_string();
            if line.ends_with('\r') {
                line.pop();
            }
            state.pending_line.drain(..newline_idx + 1);
            if let Some(event) = self.parse_structured_event_line(&line)? {
                events.push(event);
            }
        }

        // Opportunistic parse for providers that emit structured events without trailing newline.
        if !state.pending_line.trim().is_empty() {
            if let Some(event) = self.parse_structured_event_line(state.pending_line.trim())? {
                events.push(event);
                state.pending_line.clear();
            }
        }

        Ok(events)
    }

    fn flush_stream(&self, state: &mut ProviderParseState) -> Result<Vec<ProviderStructuredEvent>> {
        if state.pending_line.is_empty() {
            return Ok(Vec::new());
        }
        let line = std::mem::take(&mut state.pending_line);
        Ok(self
            .parse_structured_event_line(&line)?
            .into_iter()
            .collect())
    }

    fn build_status_snapshot(
        &self,
        session: &ManagedSession,
        latest_output: Option<&str>,
    ) -> ProviderStatusSnapshot;

    fn supports_terminal_attach(&self) -> bool {
        true
    }
}

pub fn adapter_for(provider: &str) -> Box<dyn ProviderAdapter> {
    match provider.trim().to_lowercase().as_str() {
        "opencode" => Box::new(opencode::OpenCodeAdapter::new()),
        _ => Box::new(generic::GenericCliAdapter::new(provider)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_for_unknown_provider_uses_generic_adapter() {
        let adapter = adapter_for("claude");
        assert_eq!(adapter.provider_name(), "claude");
    }

    #[test]
    fn parse_stream_chunk_buffers_partial_lines() {
        struct LineAdapter;
        impl ProviderAdapter for LineAdapter {
            fn provider_name(&self) -> &str {
                "line"
            }

            fn spawn_session(&self, _request: &StartSessionRequest) -> Result<ProviderSpawnSpec> {
                Ok(ProviderSpawnSpec {
                    command: "line".to_string(),
                    args: Vec::new(),
                    cwd: None,
                })
            }

            fn parse_structured_event_line(
                &self,
                line: &str,
            ) -> Result<Option<ProviderStructuredEvent>> {
                if !matches!(line.trim(), "active" | "stalled") {
                    return Ok(None);
                }
                Ok(Some(ProviderStructuredEvent::SessionStatus {
                    status: line.trim().to_string(),
                    reason: None,
                }))
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
        }

        let adapter = LineAdapter;
        let mut state = ProviderParseState::default();

        let first = adapter
            .parse_stream_chunk("act", &mut state)
            .expect("parse first");
        assert!(first.is_empty());

        let second = adapter
            .parse_stream_chunk("ive\nstalled\n", &mut state)
            .expect("parse second");
        assert_eq!(second.len(), 2);

        match &second[0] {
            ProviderStructuredEvent::SessionStatus { status, .. } => assert_eq!(status, "active"),
            _ => panic!("expected status event"),
        }
        match &second[1] {
            ProviderStructuredEvent::SessionStatus { status, .. } => {
                assert_eq!(status, "stalled")
            }
            _ => panic!("expected status event"),
        }
    }
}
