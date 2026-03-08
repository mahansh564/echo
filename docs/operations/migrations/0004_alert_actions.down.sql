-- Manual rollback script for 0004_alert_actions.sql
-- SQLite down migration by table recreation to remove added columns.

BEGIN TRANSACTION;

DROP INDEX IF EXISTS idx_session_alerts_visibility;
DROP INDEX IF EXISTS idx_session_alerts_agent_visibility;

ALTER TABLE session_alerts RENAME TO session_alerts_new;

CREATE TABLE session_alerts (
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

INSERT INTO session_alerts (
  id,
  session_id,
  agent_id,
  severity,
  reason,
  message,
  requires_ack,
  acknowledged_at,
  resolved_at,
  created_at,
  updated_at
)
SELECT
  id,
  session_id,
  agent_id,
  severity,
  reason,
  message,
  requires_ack,
  acknowledged_at,
  resolved_at,
  created_at,
  updated_at
FROM session_alerts_new;

DROP TABLE session_alerts_new;

CREATE INDEX IF NOT EXISTS idx_session_alerts_session_open
  ON session_alerts(session_id, resolved_at, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_alerts_agent_open
  ON session_alerts(agent_id, resolved_at, created_at DESC);

COMMIT;
