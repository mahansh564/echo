# Echo Agent Orchestration Roadmap (Desktop, Voice-First, OpenCode-First)

## Summary

Build on the existing Tauri + Svelte foundation and evolve it into a production orchestration console with:

- A vertically scrollable multi-agent runtime list.
- Voice-driven status and control flows targeting specific agents by name or index.
- Structured detection and surfacing of input-needed states.
- Full interactive terminal attach (including TUI apps) with optional popout terminal workflow.

This roadmap assumes a v1 target of up to 10 concurrent active sessions and prioritizes OpenCode with a provider adapter interface for future providers.

## Current Baseline

- Rust backend already has agents, managed sessions, PTY session start/stop, session events, and terminal input/output.
- Voice pipeline already supports wake-word flow, text processing, intent parse, and command execution.
- Svelte UI already shows agents, sessions, and a basic terminal output/input panel.
- Main gap is productization: richer runtime model, robust eventing, structured input-needed detection, real terminal attach UX, and command-quality voice controls.

## Implementation Status (As Of 2026-03-04)

- Phase 1 is in progress and substantially implemented.
- Phase 0 contract freeze artifacts are now documented in `docs/contracts/runtime-v1.md`.
- Completed in code:
  - Added migration `0003_agent_runtime_phase1.sql` with new runtime columns and `session_alerts`.
  - Added `AgentRow` and `SessionAlert` DB models and query methods.
  - Added Tauri commands:
    - `list_agent_rows_cmd`
    - `list_session_alerts_cmd`
    - `acknowledge_session_alert_cmd`
    - `resolve_session_alert_cmd`
  - Wired frontend to use `list_agent_rows_cmd` and unresolved alerts.
  - Added alert actions in UI (acknowledge/resolve) and terminal focus from alert.
  - Added migration compatibility test for legacy DB upgrade on `Db::connect`.
  - `npm run check` and `cargo test` are currently passing.
- Still pending in Phase 1:
  - None from the original Phase 1 checklist; rollback strategy documented in `docs/operations/migrations/`.

## Public APIs / Interface Changes

### Database Schema

- `agents`: add `provider`, `display_order`, `attention_state`, `active_session_id`, `last_input_required_at`.
- `managed_sessions`: add `needs_input`, `input_reason`, `last_activity_at`, `transport`, `attach_count`.
- Add `session_alerts` table:
  - `session_id`
  - `severity`
  - `reason`
  - `message`
  - `requires_ack`
  - timestamps

### Provider Abstraction

- Introduce `ProviderAdapter` trait with:
  - `spawn_session`
  - `parse_structured_event`
  - `build_status_snapshot`
  - `supports_terminal_attach`
- Implement `OpenCodeAdapter` first.

### Tauri Command Surface

- `create_agent_cmd(name, provider, task_id?)`
- `list_agent_rows_cmd(limit?, cursor?)`
- `start_agent_session_cmd(agent_id, launch_profile?)`
- `stop_agent_session_cmd(session_id)`
- `list_session_alerts_cmd(agent_id?, unresolved_only?)`
- `acknowledge_session_alert_cmd(alert_id)`
- `snooze_session_alert_cmd(alert_id, duration_minutes?)`
- `escalate_session_alert_cmd(alert_id)`
- `resolve_session_alert_cmd(alert_id)`
- `send_terminal_input_cmd(session_id, input)`
- `resize_terminal_cmd(session_id, cols, rows)`
- `get_terminal_output_cmd(session_id, cursor?)`

### Event Contracts

- `agent_runtime_updated`
- `agent_attention_updated`
- `session_alert_created`
- `session_alert_resolved`
- `terminal_chunk`
- `voice_action_executed`
- `voice_status_reply`

### Frontend Types

- `AgentRow`
- `SessionRuntime`
- `SessionAlert`
- `VoiceAction`
- `TerminalViewportState`

## Big TODO by Phases

### Phase 0 - Architecture and Contract Freeze

- [x] Write architecture doc with runtime boundaries: UI store, session supervisor, provider adapter, voice router.
- [x] Define canonical state machine for session statuses (`waking`, `active`, `stalled`, `needs_input`, `ended`, `failed`).
- [x] Define alert taxonomy (`approval_needed`, `auth_needed`, `tool_confirmation`, `input_prompt`, `unknown`).
- [x] Finalize event payload schemas and naming.
- [x] Add acceptance checklist for v1 demo and production readiness.

### Phase 1 - Data Model and Migration Layer

- [x] Add SQL migration for new agent/session/alert columns and tables.
- [x] Backfill defaults (`attention_state='ok'`, `needs_input=false`) and maintain compatibility with old rows.
- [x] Add DB query methods for `AgentRow` and unresolved alerts.
- [x] Add DB tests for lifecycle and alert-ack flows.
- [x] Ensure existing startup path safely upgrades legacy schema on connect.
- [x] Add explicit rollback/down-migration strategy for safe reversibility.

### Phase 2 - Session Supervisor + OpenCode Adapter

- [x] Refactor terminal runtime into a supervisor actor owning session lifecycle and event fanout.
- [x] Implement `OpenCodeAdapter` with structured event parsing pipeline.
- [x] Map provider events into session state + alerts + heartbeat updates.
- [x] Emit consistent runtime events for UI and voice.
- [x] Add failure handling for adapter parse errors and session orphan cleanup.

### Phase 3 - Multi-Agent Vertical UI

- [x] Replace current list/table with a vertically scrollable agent card rail.
- [x] Each card shows: agent identity, provider, task, session status, last activity, input-needed badge.
- [x] Add clear "Needs input" affordances and quick actions (`Attach`, `Reply`, `Acknowledge`).
- [x] Keep detail pane for focused agent and session timeline.
- [x] Add keyboard navigation and focus sync with voice targeting (`agent 1`, `agent alpha`).

### Phase 4 - Voice Command Surface (Hybrid Wake + Push-to-Talk)

- [x] Add push-to-talk command path in addition to wake detection.
- [x] Upgrade intent schema for deterministic actions:
  - `status_overview`
  - `status_agent`
  - `start_session`
  - `stop_session`
  - `attach_agent`
  - `send_input`
  - `list_input_needed`
- [x] Implement deterministic resolver for name + index aliases before LLM fallback.
- [x] Add spoken status summaries for alerts and direct query responses.
- [x] Add guardrails for destructive/ambiguous commands (require confirmation intent).

### Phase 5 - Full Terminal Attach Experience

- [x] Replace plain `<pre>` terminal with PTY-capable terminal widget supporting ANSI, alternate screen, resize, and raw input.
- [x] Stream incremental output chunks with cursoring/backpressure instead of full-buffer polling.
- [x] Implement attach/detach semantics so an agent session can be interactively controlled in-app.
- [x] Add popout terminal path for focused full-screen work while preserving shared session state.
- [x] Validate with TUI-heavy workflows (`vim`, `less`, REPLs, test runners).

### Phase 6 - Input-Needed Workflow Completion

- [x] Persist structured alerts and tie them to sessions/agents.
- [x] Promote alert state to agent-level `attention_state`.
- [x] Provide command palette + voice queries for unresolved inputs.
- [x] Add ack/snooze/escalate actions.
- [ ] Add optional voice summary loop (example: "2 agents need input").

### Phase 7 - Reliability, Perf, and Observability

- [ ] Add per-session ring buffer limits and memory caps for 10 active sessions.
- [ ] Add structured logs and metrics for session churn, alert latency, and voice command success.
- [ ] Add reconnect/reload behavior for UI listeners and terminal stream cursors.
- [ ] Add robust error surfaces in UI (adapter down, model down, mic unavailable).
- [ ] Hardening pass for race conditions around stop/restart/attach.

### Phase 8 - Verification and Release

- [ ] End-to-end test matrix for agent lifecycle + voice control + terminal attach.
- [ ] Manual QA scripts for the 10-agent load scenario.
- [ ] Release checklist with migration safety, rollback, and feature flagging.
- [ ] Ship v1 docs: setup, provider config, voice setup, troubleshooting.
- [ ] Capture post-v1 backlog for multi-provider expansion.

## Test Cases and Scenarios

- Agent list: 10 active sessions render smoothly, live updates visible, no stale card states.
- Voice targeting: "Status of agent 3" and "Status of Agent Alpha" resolve correctly.
- Voice command execution: "Tell agent 2 to run tests" routes input to correct session.
- Input-needed detection: structured provider prompt creates alert quickly and marks badge.
- Attach experience: user can attach and interact with full-screen TUI without broken rendering.
- Session lifecycle: stop/restart updates DB and UI consistently; no zombie runtime entries.
- Failure mode: provider/parser/mic failures emit clear UI state and recover cleanly.

## Assumptions and Defaults (Locked)

- Single desktop app is the primary v1 surface (Tauri + Svelte).
- OpenCode-first with adapter architecture for future providers.
- Structured protocol is used for input-needed detection (not regex-only heuristics).
- Voice is hybrid: wake word plus push-to-talk fallback.
- Voice targeting supports both agent name and index aliases.
- V1 reliability target is up to 10 concurrent active sessions.
- Attach UX includes in-app full PTY terminal plus optional popout workflow.
- Input-needed notification is both UI badge and voice summary.
