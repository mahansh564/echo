# Runtime V1 Architecture and Contract (Phase 0 Freeze)

Status: Accepted for implementation as of 2026-03-04.
Scope: Desktop app runtime contract for Tauri (Rust) + Svelte UI, OpenCode-first.

## 1) Runtime Boundaries

### UI Store (Svelte)
Responsibilities:
- Hold `AgentRow`, `SessionAlert`, selected agent/session, and terminal viewport state.
- Subscribe to runtime events and apply idempotent merges.
- Trigger command calls for user actions (`attach`, `ack`, `resolve`, `send_input`, etc).

Non-responsibilities:
- No provider-specific parsing.
- No direct DB writes.

### Session Supervisor (Rust backend runtime)
Responsibilities:
- Own in-memory lifecycle for active sessions.
- Coordinate PTY process, heartbeat/last activity, and attach count.
- Fan out normalized runtime events to UI + voice router.
- Enforce state transitions.

Non-responsibilities:
- No provider-specific protocol semantics beyond adapter outputs.

### Provider Adapter (Rust trait)
Responsibilities:
- Spawn provider session process.
- Parse provider output into normalized structured events.
- Produce runtime snapshots + input-needed signals.

Non-responsibilities:
- No event transport to UI.
- No voice intent decisions.

### Voice Router (Rust voice pipeline)
Responsibilities:
- Resolve voice intents into deterministic actions.
- Resolve agent name/index aliases.
- Request/consume runtime status and emit spoken summaries.

Non-responsibilities:
- No DB schema concerns.
- No provider protocol parsing.

## 2) Canonical Session State Machine

Canonical statuses:
- `waking`: process started, waiting for readiness.
- `active`: session healthy and interactive.
- `stalled`: no useful progress/heartbeat in expected window.
- `needs_input`: blocked on explicit user input.
- `ended`: graceful completion or stop.
- `failed`: unrecoverable process/adapter error.

Allowed transitions:
- `waking -> active`
- `waking -> failed`
- `active -> stalled`
- `active -> needs_input`
- `active -> ended`
- `active -> failed`
- `stalled -> active`
- `stalled -> needs_input`
- `stalled -> ended`
- `stalled -> failed`
- `needs_input -> active`
- `needs_input -> ended`
- `needs_input -> failed`

Terminal states:
- `ended`
- `failed`

Rules:
- Any state transition must update `managed_sessions.updated_at`.
- Transitions into `active`, `stalled`, and `needs_input` must refresh `last_activity_at`.
- Transitions into `ended`/`failed` must set `ended_at` if null.

## 3) Alert Taxonomy

Canonical `reason` values:
- `approval_needed`: user approval required before action.
- `auth_needed`: authentication token/login/session required.
- `tool_confirmation`: tool requested explicit confirmation.
- `input_prompt`: model/runtime requested free-form user input.
- `unknown`: fallback for non-classified alerts.

Severity guidance:
- `info`: advisory, no interruption.
- `warning`: action recommended soon.
- `critical`: immediate action required.

Alert lifecycle:
- Created unresolved (`resolved_at = NULL`).
- May be acknowledged (`acknowledged_at` set) independently of resolution.
- May be snoozed (`snoozed_until`) to suppress unresolved surfacing until expiry.
- May be escalated (`severity=critical`, `escalated_at`, incrementing `escalation_count`).
- Resolved when no longer actionable (`resolved_at` set).

## 4) Event Naming and Payload Schemas

Naming convention:
- Lowercase snake case.
- Domain-first where possible (`agent_*`, `session_*`, `terminal_*`, `voice_*`).

Required events and payloads:

### `agent_runtime_updated`
```json
{
  "agent_id": 12,
  "active_session_id": 77,
  "status": "needs_input",
  "attention_state": "needs_input",
  "last_activity_at": "2026-03-04T19:22:00Z"
}
```

### `agent_attention_updated`
```json
{
  "agent_id": 12,
  "attention_state": "needs_input",
  "unresolved_alert_count": 2,
  "last_input_required_at": "2026-03-04T19:21:57Z"
}
```

### `session_alert_created`
```json
{
  "alert_id": 314,
  "session_id": 77,
  "agent_id": 12,
  "severity": "warning",
  "reason": "input_prompt",
  "message": "Please confirm deploy target",
  "requires_ack": true,
  "created_at": "2026-03-04T19:21:57Z"
}
```

### `session_alert_resolved`
```json
{
  "alert_id": 314,
  "session_id": 77,
  "agent_id": 12,
  "resolved_at": "2026-03-04T19:24:10Z"
}
```

### `terminal_chunk`
```json
{
  "session_id": 77,
  "cursor": 9812,
  "chunk": "...ansi bytes...",
  "is_delta": true,
  "at": "2026-03-04T19:24:13Z"
}
```

### `voice_action_executed`
```json
{
  "action": "send_input",
  "target_agent_id": 12,
  "target_session_id": 77,
  "text": "run tests",
  "result": "ok",
  "at": "2026-03-04T19:24:25Z"
}
```

### `voice_status_reply`
```json
{
  "request_type": "status_agent",
  "target_agent_id": 12,
  "summary": "Agent Alpha is active and has 1 unresolved input prompt.",
  "at": "2026-03-04T19:24:26Z"
}
```

Compatibility rule:
- Additive payload evolution only in v1. Do not rename or remove existing keys without a version gate.

## 5) Acceptance Checklist (V1 Demo + Production Readiness)

### Demo acceptance
- 10 active agents render in vertical rail with live updates.
- Voice targeting by index and name resolves to correct agent.
- `send_input` via voice routes to intended session.
- Input-needed alert appears within expected latency and can be acknowledged/resolved.
- PTY attach supports interactive workflows without output corruption.

### Production readiness
- Migration rollback procedure documented and validated.
- Startup handles legacy DB upgrades safely.
- Runtime events are stable and backward compatible for UI listeners.
- Session stop/restart has no zombie processes and no orphan DB rows.
- Error states are surfaced for provider down/mic unavailable/adapter parse failures.
- Test suite includes DB lifecycle + alerts + runtime event smoke coverage.

## 6) Rollback and Down-Migration Strategy (Phase 1)

Primary policy:
- For local/dev: allow schema rollback via manual down script.
- For production: prefer restore-from-backup + forward-fix migration, unless rollback is explicitly approved.

Artifacts:
- Up migration: `src-tauri/migrations/0003_agent_runtime_phase1.sql`
- Down script: `docs/operations/migrations/0003_agent_runtime_phase1.down.sql`

Rollback procedure:
1. Stop app processes that can write to DB.
2. Create SQLite backup file before changes.
3. Execute down script in a transaction on target DB.
4. Start app and run a smoke test (`cargo test db::tests::migration_compatibility_upgrades_legacy_schema`).
5. If rollback fails, restore the backup and move to forward-fix path.

Risk notes:
- Down migration drops `session_alerts` and removes Phase 1 columns.
- Data in dropped columns/tables is unrecoverable without prior backup.
