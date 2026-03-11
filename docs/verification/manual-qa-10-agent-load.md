# Manual QA Scripts: 10-Agent Load Scenario (Phase 8)

Status: Adopted for v1 verification as of 2026-03-11.  
Scope: Manual validation of UI/runtime behavior at 10 concurrent active sessions.

## Objective

Validate that the desktop app remains correct and usable with 10 active agent sessions, including:

- Vertical agent rail stability and live updates.
- Voice targeting and command routing.
- Terminal attach/detach, popout, and interactive control.
- Recovery behavior after stop/restart/reload flows.

## Preconditions

1. Build and checks pass:
   - `cd src-tauri && cargo test --quiet`
   - `npm run check`
2. Launch app in development mode:
   - `npm run tauri dev`
3. Voice wake detection is running by default at app startup, and microphone permission is granted for voice scripts.
4. Tester can open the in-app terminal popout window.

## Evidence To Capture

- Screen recording for the full run.
- Timestamped notes for each script (`Start`, `End`, `Result`, `Notes`).
- Any console/backend error text for failed steps.

## Script Index

| Script ID | Name | Duration | Pass/Fail |
| --- | --- | --- | --- |
| QA10-00 | Environment preflight | 5 min | |
| QA10-01 | Seed 10 agents and tasks | 10 min | |
| QA10-02 | Start and stabilize 10 active sessions | 10 min | |
| QA10-03 | Attach/detach and popout stress | 10 min | |
| QA10-04 | Voice targeting and routing sweep | 10 min | |
| QA10-05 | Input-needed and alert workflow | 10 min | |
| QA10-06 | Stop/restart race pass | 10 min | |
| QA10-07 | Reload/reconnect resilience | 5 min | |
| QA10-08 | Teardown and post-run verification | 5 min | |

## QA10-00: Environment Preflight

Steps:
1. Confirm app starts without startup DB migration errors.
2. Open the main view and verify no blocking runtime issue banner is present.
3. Open command palette and confirm it renders and closes normally.
4. Open and close terminal popout once to confirm window creation works.

Expected:
- No fatal startup/runtime errors.
- Main view, command palette, and popout all function.

## QA10-01: Seed 10 Agents and Tasks

Steps:
1. Create 10 tasks named `Load Task 01` through `Load Task 10`.
2. Create 10 agents named `Load Agent 01` through `Load Agent 10`.
3. Assign one task to each agent in order.
4. Verify each agent card shows the expected agent/task pairing.

Expected:
- Exactly 10 agents visible.
- Display order is stable and matches creation order.
- No duplicate/missing agent rows.

## QA10-02: Start and Stabilize 10 Active Sessions

Steps:
1. Start one session for each of the 10 agents.
2. Wait until all sessions report open-state (`waking`, `active`, `stalled`, or `needs_input`).
3. Leave app running for 5 minutes while observing updates.
4. Confirm the agent rail remains scrollable and responsive.

Expected:
- 10 sessions become active/open without app freeze.
- Live updates continue (status/snippet/activity changes).
- No stale card rows or duplicated active-session ownership for an agent.

## QA10-03: Attach/Detach and Popout Stress

Steps:
1. Attach to agent sessions 1 through 10 sequentially from the main view.
2. For each attach, send one line of input and verify output appears.
3. Open popout on 3 different sessions and confirm interaction works in popout.
4. Rapidly detach/attach across at least 3 sessions (5 cycles each).

Expected:
- Attach only succeeds for running sessions.
- Terminal remains interactive and output streams correctly.
- Attach count does not drift upward after full detach cycles.
- No corrupted TUI rendering or frozen terminal widget.

## QA10-04: Voice Targeting and Routing Sweep

Steps:
1. Speak: `Status of agent 3`.
2. Speak: `Status of agent alpha`.
3. Speak: `Tell agent 2 to echo voice-route-ok`.
4. Speak an ambiguous/destructive command without confirmation.
5. Repeat with confirmation phrase.

Expected:
- Index and alias targeting resolve deterministically.
- Input is routed to the intended session only.
- Confirmation guardrails block risky/ambiguous command until confirmed.

## QA10-05: Input-Needed and Alert Workflow

Steps:
1. Drive at least 2 sessions into `needs_input` or equivalent unresolved alert state.
2. Verify cards show input-needed badges/affordances.
3. Use command palette to list unresolved alerts and focus one target session.
4. Acknowledge one alert and resolve another alert.
5. Verify attention state updates on corresponding agents.

Expected:
- Alerts appear promptly and are queryable from command palette.
- Acknowledge/resolve transitions update UI state without reload.
- Agent attention reflects unresolved critical/open input state.

## QA10-06: Stop/Restart Race Pass

Steps:
1. Pick 3 agents with active sessions.
2. For each, execute rapid sequence: `stop -> start -> attach -> stop -> start`.
3. Repeat the sequence twice per agent.
4. Check that final state has at most one open session per test agent.

Expected:
- No zombie sessions.
- Stop is tolerant of repeated calls.
- Restart converges to a single active session for each agent.
- Attach never succeeds against an ended runtime.

## QA10-07: Reload/Reconnect Resilience

Steps:
1. With 10 sessions active, reload the UI window.
2. Confirm listeners reconnect and terminal cursor streaming resumes.
3. Re-open terminal popout and switch target session.
4. Verify unresolved alerts and session statuses are still accurate.

Expected:
- Session state rehydrates correctly after reload.
- Terminal output streaming resumes from current cursor.
- No lost selection synchronization between rail/detail/popup paths.

## QA10-08: Teardown and Post-Run Verification

Steps:
1. Stop all active sessions.
2. Detach any remaining attached terminals.
3. Verify no agent remains with an open session unexpectedly.
4. Save run notes and mark each script row pass/fail with evidence links.

Expected:
- All sessions terminate cleanly.
- No orphaned open session state remains in UI.
- QA report is complete and reproducible.

## Result Template

Use one line per script:

`<script-id> | <start time> | <end time> | <pass/fail> | <notes/evidence>`

Example:

`QA10-03 | 14:05 | 14:16 | PASS | popout attach verified on sessions #4/#7/#9`
