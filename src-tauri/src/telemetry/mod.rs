use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySnapshot {
    pub session_start_total: u64,
    pub session_start_error_total: u64,
    pub session_end_total: u64,
    pub session_user_stop_total: u64,
    pub voice_command_total: u64,
    pub voice_command_success_total: u64,
    pub voice_command_error_total: u64,
    pub alert_resolved_total: u64,
    pub alert_resolution_latency_ms_avg: Option<u64>,
    pub alert_resolution_latency_ms_max: Option<u64>,
    pub session_starts_by_provider: BTreeMap<String, u64>,
    pub voice_commands_by_action: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
struct TelemetryInner {
    session_start_total: u64,
    session_start_error_total: u64,
    session_end_total: u64,
    session_user_stop_total: u64,
    voice_command_total: u64,
    voice_command_success_total: u64,
    voice_command_error_total: u64,
    alert_resolved_total: u64,
    alert_resolution_latency_ms_sum: u64,
    alert_resolution_latency_samples: u64,
    alert_resolution_latency_ms_max: Option<u64>,
    session_starts_by_provider: BTreeMap<String, u64>,
    voice_commands_by_action: BTreeMap<String, u64>,
}

#[derive(Clone, Default)]
pub struct Telemetry {
    inner: Arc<Mutex<TelemetryInner>>,
}

impl Telemetry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        let inner = self.inner.lock().expect("telemetry lock poisoned");
        let avg = if inner.alert_resolution_latency_samples == 0 {
            None
        } else {
            Some(inner.alert_resolution_latency_ms_sum / inner.alert_resolution_latency_samples)
        };
        TelemetrySnapshot {
            session_start_total: inner.session_start_total,
            session_start_error_total: inner.session_start_error_total,
            session_end_total: inner.session_end_total,
            session_user_stop_total: inner.session_user_stop_total,
            voice_command_total: inner.voice_command_total,
            voice_command_success_total: inner.voice_command_success_total,
            voice_command_error_total: inner.voice_command_error_total,
            alert_resolved_total: inner.alert_resolved_total,
            alert_resolution_latency_ms_avg: avg,
            alert_resolution_latency_ms_max: inner.alert_resolution_latency_ms_max,
            session_starts_by_provider: inner.session_starts_by_provider.clone(),
            voice_commands_by_action: inner.voice_commands_by_action.clone(),
        }
    }

    pub fn record_session_started(
        &self,
        session_id: i64,
        agent_id: Option<i64>,
        provider: &str,
        source: &str,
    ) {
        {
            let mut inner = self.inner.lock().expect("telemetry lock poisoned");
            inner.session_start_total += 1;
            *inner
                .session_starts_by_provider
                .entry(provider.to_string())
                .or_insert(0) += 1;
        }
        log_event(
            "session_churn",
            serde_json::json!({
                "phase": "started",
                "sessionId": session_id,
                "agentId": agent_id,
                "provider": provider,
                "source": source,
            }),
        );
    }

    pub fn record_session_start_failed(
        &self,
        session_id: Option<i64>,
        agent_id: Option<i64>,
        provider: &str,
        source: &str,
        reason: &str,
    ) {
        {
            let mut inner = self.inner.lock().expect("telemetry lock poisoned");
            inner.session_start_error_total += 1;
        }
        log_event(
            "session_churn",
            serde_json::json!({
                "phase": "start_failed",
                "sessionId": session_id,
                "agentId": agent_id,
                "provider": provider,
                "source": source,
                "reason": reason,
            }),
        );
    }

    pub fn record_session_ended(
        &self,
        session_id: i64,
        agent_id: Option<i64>,
        reason: &str,
        source: &str,
    ) {
        {
            let mut inner = self.inner.lock().expect("telemetry lock poisoned");
            inner.session_end_total += 1;
        }
        log_event(
            "session_churn",
            serde_json::json!({
                "phase": "ended",
                "sessionId": session_id,
                "agentId": agent_id,
                "reason": reason,
                "source": source,
            }),
        );
    }

    pub fn record_session_user_stop(&self, session_id: i64, agent_id: Option<i64>, source: &str) {
        {
            let mut inner = self.inner.lock().expect("telemetry lock poisoned");
            inner.session_end_total += 1;
            inner.session_user_stop_total += 1;
        }
        log_event(
            "session_churn",
            serde_json::json!({
                "phase": "user_stopped",
                "sessionId": session_id,
                "agentId": agent_id,
                "source": source,
            }),
        );
    }

    pub fn record_session_stop_failed(&self, session_id: i64, source: &str, reason: &str) {
        log_event(
            "session_churn",
            serde_json::json!({
                "phase": "stop_failed",
                "sessionId": session_id,
                "source": source,
                "reason": reason,
            }),
        );
    }

    pub fn record_voice_command(
        &self,
        action: &str,
        outcome: &str,
        target_agent_id: Option<i64>,
        target_session_id: Option<i64>,
    ) {
        {
            let mut inner = self.inner.lock().expect("telemetry lock poisoned");
            inner.voice_command_total += 1;
            if outcome == "ok" {
                inner.voice_command_success_total += 1;
            } else {
                inner.voice_command_error_total += 1;
            }
            *inner
                .voice_commands_by_action
                .entry(action.to_string())
                .or_insert(0) += 1;
        }
        log_event(
            "voice_command",
            serde_json::json!({
                "action": action,
                "outcome": outcome,
                "targetAgentId": target_agent_id,
                "targetSessionId": target_session_id,
            }),
        );
    }

    pub fn record_alert_resolved(
        &self,
        alert_id: i64,
        session_id: i64,
        agent_id: Option<i64>,
        latency_ms: Option<i64>,
    ) {
        let latency_u64 = latency_ms.and_then(|value| u64::try_from(value).ok());
        {
            let mut inner = self.inner.lock().expect("telemetry lock poisoned");
            inner.alert_resolved_total += 1;
            if let Some(value) = latency_u64 {
                inner.alert_resolution_latency_ms_sum += value;
                inner.alert_resolution_latency_samples += 1;
                inner.alert_resolution_latency_ms_max = Some(
                    inner
                        .alert_resolution_latency_ms_max
                        .map(|current| current.max(value))
                        .unwrap_or(value),
                );
            }
        }
        log_event(
            "alert_latency",
            serde_json::json!({
                "phase": "resolved",
                "alertId": alert_id,
                "sessionId": session_id,
                "agentId": agent_id,
                "resolutionLatencyMs": latency_u64,
            }),
        );
    }
}

fn log_event(event: &str, payload: serde_json::Value) {
    let line = serde_json::json!({
        "tsMs": now_unix_ms(),
        "event": event,
        "payload": payload,
    });
    eprintln!("{}", line);
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::Telemetry;

    #[test]
    fn telemetry_snapshot_rolls_up_metrics() {
        let telemetry = Telemetry::new();
        telemetry.record_session_started(1, Some(5), "opencode", "test");
        telemetry.record_session_start_failed(None, Some(5), "opencode", "test", "spawn failed");
        telemetry.record_session_user_stop(1, Some(5), "test");
        telemetry.record_voice_command("status_overview", "ok", Some(5), Some(1));
        telemetry.record_voice_command("stop_session", "error", Some(5), Some(1));
        telemetry.record_alert_resolved(99, 1, Some(5), Some(1200));

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.session_start_total, 1);
        assert_eq!(snapshot.session_start_error_total, 1);
        assert_eq!(snapshot.session_end_total, 1);
        assert_eq!(snapshot.session_user_stop_total, 1);
        assert_eq!(snapshot.voice_command_total, 2);
        assert_eq!(snapshot.voice_command_success_total, 1);
        assert_eq!(snapshot.voice_command_error_total, 1);
        assert_eq!(snapshot.alert_resolved_total, 1);
        assert_eq!(snapshot.alert_resolution_latency_ms_avg, Some(1200));
        assert_eq!(snapshot.alert_resolution_latency_ms_max, Some(1200));
        assert_eq!(
            snapshot.session_starts_by_provider.get("opencode"),
            Some(&1)
        );
        assert_eq!(
            snapshot.voice_commands_by_action.get("status_overview"),
            Some(&1)
        );
    }
}
