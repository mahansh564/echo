# End-to-End Test Matrix (Phase 8)

Status: Adopted for v1 verification as of 2026-03-11.
Scope: Agent lifecycle + voice control + terminal attach workflows on desktop (Tauri + Svelte).

## Goal

Define a single source of truth for end-to-end verification coverage across:

- Agent/session lifecycle transitions.
- Voice command targeting and guardrails.
- Terminal attach, interaction, detach, and race handling.

## How To Execute

1. Run automated backend coverage:
   - `cd src-tauri && cargo test --quiet`
2. Run frontend checks:
   - `npm run check`
3. Execute manual scenarios marked `Manual` in the matrix below.
   - Manual script pack: `docs/verification/manual-qa-10-agent-load.md`
4. Record pass/fail and notes in the release checklist before shipping.

## Matrix

| ID | Area | Scenario | Mode | Coverage Source | Pass Criteria |
| --- | --- | --- | --- | --- | --- |
| E2E-LC-01 | Lifecycle | Create session row, transition to `active`, heartbeat/event persistence | Automated | `db::tests::managed_session_lifecycle_persists` | Session persists with `status=active`, `transport=pty`, heartbeat/event rows present |
| E2E-LC-02 | Lifecycle | Startup orphan reconciliation marks DB-open/non-runtime sessions as `failed` | Automated | `terminal::tests::reconcile_orphan_sessions_marks_open_rows_failed` | Open orphan row becomes `failed` with no runtime handle |
| E2E-LC-03 | Lifecycle | Stop is idempotent when runtime handle is missing but DB row is still open | Automated | `terminal::tests::stop_session_is_idempotent_for_db_open_rows_without_runtime` | Stop call returns success and row converges to `ended` |
| E2E-LC-04 | Lifecycle | Restart safety: starting new session for an agent first stops prior active session | Automated + Manual | `terminal::SessionSupervisor::start_session` pre-stop logic; manual verification via UI/voice | Only one open session remains active for an agent after restart |
| E2E-VC-01 | Voice | Deterministic voice targeting by index (`agent 3`) | Automated | `voice::tests::intent_parses`, `voice::tests::status_resolver_prefers_deterministic_index` | Parsed action is `status_agent` with index hint and correct resolution path |
| E2E-VC-02 | Voice | Deterministic voice targeting by alias (`agent alpha`) | Automated | `voice::tests::intent_parses_nato_alias_target`, `voice::tests::status_resolver_prefers_deterministic_alias`, `voice::tests::router_resolves_agent_index_and_alias` | Alias resolves to expected agent index/id |
| E2E-VC-03 | Voice | Destructive/ambiguous command guardrails require confirmation | Automated + Manual | `voice::intent` fallback/confirmation logic, `voice::router::execute_command` confirmation paths | Command is blocked until `confirmed=true`; prompt/confirmation response is emitted |
| E2E-VC-04 | Voice | `send_input` routes to active session and clears `needs_input` | Manual | Run with push-to-talk/wake flow against active session | Input appears in target terminal, `needs_input` badge clears, session remains interactive |
| E2E-TA-01 | Terminal Attach | Attach/detach updates attach counter and never drops below zero | Automated | `db::tests::terminal_attach_detach_updates_attach_count` | Attach increments, detach decrements, floor at `0` |
| E2E-TA-02 | Terminal Attach | Attach is rejected if runtime session is missing | Automated | `terminal::tests::attach_session_rejects_when_runtime_is_missing` | Attach returns runtime-unavailable error and does not leave stale attach state |
| E2E-TA-03 | Terminal Attach | Interactive attach supports incremental input/output loop | Automated | `terminal::tests::repl_like_workflow_accepts_incremental_input` | Attached session receives input and returns expected streamed output |
| E2E-TA-04 | Terminal Attach | PTY resize and alternate-screen behavior for TUI workflows | Automated | `terminal::tests::resize_emits_winch_for_tui_processes`, `terminal::tests::tui_alt_screen_sequences_are_preserved` | Resize propagates; ANSI alternate-screen sequences are preserved |
| E2E-TA-05 | Terminal Attach | Stop/attach race hardening under rapid user actions | Automated + Manual | `terminal::SessionSupervisor::attach_session` runtime pre/post checks | No successful attach to dead session; no stuck `attach_count`; session state converges |

## Manual Scenario Definitions

### MS-01: Voice Start -> Attach -> Send Input -> Stop

1. Create/select an agent in the main UI.
2. Voice: start session for the agent.
3. Attach to the agent terminal.
4. Voice: send input (`run tests` or `echo hello`).
5. Voice: stop session with confirmation.

Expected:
- Session reaches `active`, then `ended`.
- Terminal shows routed input/output for the targeted agent only.
- No orphan active session row remains after stop.

### MS-02: Rapid Restart and Reattach

1. Start an agent session.
2. Immediately start again for the same agent (or restart via UI control).
3. Attach terminal while restart is in flight.
4. Detach/attach repeatedly during the transition.

Expected:
- Exactly one active session persists for the agent.
- Attach errors only when runtime is unavailable and recover on retry.
- `attach_count` stays consistent and returns to `0` on full detach/end.

### MS-03: Alias and Index Voice Targeting

1. Ensure at least 3 agents exist with distinct names.
2. Voice: "status of agent 3".
3. Voice: "status of agent alpha".
4. Voice: ambiguous command without confirmation.
5. Repeat ambiguous command with confirmation.

Expected:
- Deterministic routing for index/alias.
- Ambiguous/destructive commands require confirmation before execution.

## Exit Criteria

Phase 8 E2E matrix is considered complete when:

- All automated coverage above is green in CI/local run.
- All manual scenarios pass on a release candidate build.
- Any failed row has an associated blocking issue before release.
