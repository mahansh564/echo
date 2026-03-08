# Migration Rollback Playbook

Use these scripts only for controlled rollback scenarios.

## Phase 1 rollback

- Up migration: `src-tauri/migrations/0003_agent_runtime_phase1.sql`
- Down script: `docs/operations/migrations/0003_agent_runtime_phase1.down.sql`

### Steps

1. Stop all app processes.
2. Backup the SQLite DB file.
3. Run the down script:

```bash
sqlite3 /path/to/app.db < docs/operations/migrations/0003_agent_runtime_phase1.down.sql
```

4. Start the app and run migration-compatibility smoke tests.

### Warning

The rollback drops `session_alerts` and Phase 1 columns from `agents` and `managed_sessions`. Always keep a backup before rollback.

## Phase 6 alert actions rollback

- Up migration: `src-tauri/migrations/0004_alert_actions.sql`
- Down script: `docs/operations/migrations/0004_alert_actions.down.sql`

### Steps

1. Stop all app processes.
2. Backup the SQLite DB file.
3. Run the down script:

```bash
sqlite3 /path/to/app.db < docs/operations/migrations/0004_alert_actions.down.sql
```

4. Start the app and run DB + command alert tests.

### Warning

This rollback removes snooze/escalation metadata (`snoozed_until`, `escalated_at`, `escalation_count`) from `session_alerts`.
