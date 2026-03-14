ALTER TABLE session_alerts ADD COLUMN message_enriched TEXT;
ALTER TABLE session_alerts ADD COLUMN message_enrichment_status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE session_alerts ADD COLUMN message_enriched_at TEXT;
ALTER TABLE session_alerts ADD COLUMN message_enrichment_error TEXT;

CREATE INDEX IF NOT EXISTS idx_session_alerts_enrichment_status
  ON session_alerts(message_enrichment_status, updated_at DESC);
