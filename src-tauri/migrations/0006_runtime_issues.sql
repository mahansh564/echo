CREATE TABLE IF NOT EXISTS runtime_issues (
  kind TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  raw_message TEXT NOT NULL,
  enriched_message TEXT,
  enrichment_status TEXT NOT NULL DEFAULT 'pending',
  enrichment_error TEXT,
  first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  seen_count INTEGER NOT NULL DEFAULT 1,
  dismissed_until TEXT,
  resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_runtime_issues_visibility
  ON runtime_issues(resolved_at, dismissed_until, last_seen_at DESC);
