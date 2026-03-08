use anyhow::Result;

use crate::{
    db::models::{ManagedSession, StartSessionRequest},
    providers::{ProviderAdapter, ProviderSpawnSpec, ProviderStatusSnapshot},
};

#[derive(Debug, Clone)]
pub struct GenericCliAdapter {
    provider: String,
}

impl GenericCliAdapter {
    pub fn new(provider: &str) -> Self {
        Self {
            provider: provider.trim().to_string(),
        }
    }

    fn provider_fallback_command(&self) -> &str {
        if self.provider.is_empty() {
            "opencode"
        } else {
            &self.provider
        }
    }
}

impl ProviderAdapter for GenericCliAdapter {
    fn provider_name(&self) -> &str {
        if self.provider.is_empty() {
            "unknown"
        } else {
            &self.provider
        }
    }

    fn spawn_session(&self, request: &StartSessionRequest) -> Result<ProviderSpawnSpec> {
        let command = if request.command.trim().is_empty() {
            self.provider_fallback_command().to_string()
        } else {
            request.command.trim().to_string()
        };

        Ok(ProviderSpawnSpec {
            command,
            args: request.args.clone(),
            cwd: request.cwd.clone(),
        })
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
