CREATE TABLE IF NOT EXISTS managed_sessions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider TEXT NOT NULL DEFAULT 'opencode',
  status TEXT NOT NULL,
  launch_command TEXT NOT NULL,
  launch_args_json TEXT NOT NULL,
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

CREATE TABLE IF NOT EXISTS session_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  message TEXT,
  payload_json TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (session_id) REFERENCES managed_sessions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_managed_sessions_status_updated
  ON managed_sessions(status, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_managed_sessions_agent
  ON managed_sessions(agent_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_managed_sessions_task
  ON managed_sessions(task_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_session_events_session_created
  ON session_events(session_id, created_at DESC);
