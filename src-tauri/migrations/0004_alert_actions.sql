ALTER TABLE session_alerts ADD COLUMN snoozed_until TEXT;
ALTER TABLE session_alerts ADD COLUMN escalated_at TEXT;
ALTER TABLE session_alerts ADD COLUMN escalation_count INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_session_alerts_agent_visibility
  ON session_alerts(agent_id, resolved_at, snoozed_until, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_alerts_visibility
  ON session_alerts(resolved_at, snoozed_until, created_at DESC);
