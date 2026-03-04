use anyhow::Result;

use crate::db::models::{ManagedSession, StartSessionRequest};

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
    fn provider_name(&self) -> &'static str;

    fn spawn_session(&self, request: &StartSessionRequest) -> Result<ProviderSpawnSpec>;

    fn parse_structured_event(&self, chunk: &str) -> Result<Option<ProviderStructuredEvent>>;

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
    match provider {
        "opencode" => Box::new(opencode::OpenCodeAdapter::new()),
        _ => Box::new(opencode::OpenCodeAdapter::new()),
    }
}
