-- Manual rollback script for 0003_agent_runtime_phase1.sql
-- SQLite does not support DROP COLUMN directly in a way compatible with older setups,
-- so we recreate affected tables and copy compatible data.

BEGIN TRANSACTION;

DROP INDEX IF EXISTS idx_session_alerts_agent_open;
DROP INDEX IF EXISTS idx_session_alerts_session_open;
DROP INDEX IF EXISTS idx_managed_sessions_needs_input;
DROP INDEX IF EXISTS idx_agents_attention_state;
DROP INDEX IF EXISTS idx_agents_display_order;

DROP TABLE IF EXISTS session_alerts;

ALTER TABLE managed_sessions RENAME TO managed_sessions_new;

CREATE TABLE managed_sessions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'waking',
  launch_command TEXT NOT NULL,
  launch_args_json TEXT NOT NULL DEFAULT '[]',
  cwd TEXT,
  pid INTEGER,
  agent_id INTEGER,
  task_id INTEGER,
  last_heartbeat_at TEXT,
  started_at TEXT,
  ended_at TEXT,
  failure_reason TEXT,
  metadata_json TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE SET NULL,
  FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE SET NULL
);

INSERT INTO managed_sessions (
  id,
  provider,
  status,
  launch_command,
  launch_args_json,
  cwd,
  pid,
  agent_id,
  task_id,
  last_heartbeat_at,
  started_at,
  ended_at,
  failure_reason,
  metadata_json,
  created_at,
  updated_at
)
SELECT
  id,
  provider,
  status,
  launch_command,
  launch_args_json,
  cwd,
  pid,
  agent_id,
  task_id,
  last_heartbeat_at,
  started_at,
  ended_at,
  failure_reason,
  metadata_json,
  created_at,
  updated_at
FROM managed_sessions_new;

DROP TABLE managed_sessions_new;

ALTER TABLE agents RENAME TO agents_new;

CREATE TABLE agents (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'idle',
  task_id INTEGER,
  last_snippet TEXT,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE SET NULL
);

INSERT INTO agents (
  id,
  name,
  state,
  task_id,
  last_snippet,
  updated_at
)
SELECT
  id,
  name,
  state,
  task_id,
  last_snippet,
  updated_at
FROM agents_new;

DROP TABLE agents_new;

COMMIT;
