ALTER TABLE agents ADD COLUMN provider TEXT NOT NULL DEFAULT 'opencode';
ALTER TABLE agents ADD COLUMN display_order INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agents ADD COLUMN attention_state TEXT NOT NULL DEFAULT 'ok';
ALTER TABLE agents ADD COLUMN active_session_id INTEGER;
ALTER TABLE agents ADD COLUMN last_input_required_at TEXT;

UPDATE agents
SET display_order = id
WHERE display_order = 0;

ALTER TABLE managed_sessions ADD COLUMN needs_input INTEGER NOT NULL DEFAULT 0;
ALTER TABLE managed_sessions ADD COLUMN input_reason TEXT;
ALTER TABLE managed_sessions ADD COLUMN last_activity_at TEXT;
ALTER TABLE managed_sessions ADD COLUMN transport TEXT NOT NULL DEFAULT 'pty';
ALTER TABLE managed_sessions ADD COLUMN attach_count INTEGER NOT NULL DEFAULT 0;

UPDATE managed_sessions
SET last_activity_at = COALESCE(last_heartbeat_at, updated_at, created_at)
WHERE last_activity_at IS NULL;

CREATE TABLE IF NOT EXISTS session_alerts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id INTEGER NOT NULL,
  agent_id INTEGER,
  severity TEXT NOT NULL DEFAULT 'info',
  reason TEXT NOT NULL,
  message TEXT NOT NULL,
  requires_ack INTEGER NOT NULL DEFAULT 1,
  acknowledged_at TEXT,
  resolved_at TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (session_id) REFERENCES managed_sessions(id) ON DELETE CASCADE,
  FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_agents_display_order
  ON agents(display_order, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_agents_attention_state
  ON agents(attention_state, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_managed_sessions_needs_input
  ON managed_sessions(needs_input, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_session_alerts_session_open
  ON session_alerts(session_id, resolved_at, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_alerts_agent_open
  ON session_alerts(agent_id, resolved_at, created_at DESC);
