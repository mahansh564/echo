#[derive(Debug, Default)]
pub struct EchoState {
    pub agents: Vec<String>,
}

impl EchoState {
    pub fn new() -> Self {
        Self { agents: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_state_starts_empty() {
        let state = EchoState::new();
        assert_eq!(state.agents.len(), 0);
    }
}
