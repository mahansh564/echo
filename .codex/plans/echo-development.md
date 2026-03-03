# Echo Voice-First Agent Orchestrator Implementation Plan

**Goal:** Build a macOS-only Tauri + SvelteKit app that provides a voice-first agent list UI with embedded terminal sessions, local SQLite storage, read-only Linear import, and local audio/LLM integration stubs wired end-to-end.

**Architecture:** Single Tauri app with a Rust backend (“Echo Core”) that owns long-running services (audio, intents, terminal PTYs, SQLite, Linear sync). Frontend is SvelteKit and communicates via Tauri commands + event bus.

**Tech Stack:** Tauri (Rust), SvelteKit, TypeScript, SQLite (via sqlx), pty (via portable-pty), Porcupine (wake word), whisper.cpp (ASR), macOS TTS, Ollama (LLM).

---

### Task 1: Scaffold Tauri + SvelteKit app

**Step 1: Run scaffold command**
Run: `npm create tauri-app@latest . -- --template svelte-kit --package-manager npm`
Expected: creates `src-tauri/`, `src/`, `package.json`

**Step 2: Install dependencies**
Run: `npm install`
Expected: node_modules installed

**Step 3: Verify dev build starts**
Run: `npm run tauri dev`
Expected: Tauri app opens with default SvelteKit page

**Step 4: Commit scaffold**
Run:
```bash
git add package.json package-lock.json src src-tauri

git commit -m "chore: scaffold tauri + sveltekit app"
```

### Task 2: Add core Rust crates and setup Echo Core skeleton

**Files:**
- Modify: `/Users/anshulmahajan/Desktop/Projects/echo/Cargo.toml`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/core/mod.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/core/events.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/core/state.rs`

**Step 1: Write failing test (Rust unit test)**
Create test in `/Users/anshulmahajan/Desktop/Projects/echo/src/core/state.rs`:
```rust
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn core_state_starts_empty() {
    let state = EchoState::new();
    assert_eq!(state.agents.len(), 0);
  }
}
```
Expected: compile fails (EchoState not defined)

**Step 2: Run test to verify it fails**
Run: `cargo test -p echo-tauri`
Expected: FAIL with missing EchoState

**Step 3: Write minimal implementation**
Add EchoState struct with minimal fields and constructor:
```rust
pub struct EchoState {
  pub agents: Vec<String>,
}

impl EchoState {
  pub fn new() -> Self { Self { agents: Vec::new() } }
}
```

**Step 4: Run test to verify it passes**
Run: `cargo test -p echo-tauri`
Expected: PASS

**Step 5: Commit**
```bash
git add src-tauri/Cargo.toml src-tauri/src/core

git commit -m "feat(core): add echo core state skeleton"
```

### Task 3: SQLite schema + migrations + DAO layer (TDD)

**Files:**
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/migrations/0001_init.sql`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/db/mod.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/db/models.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/db/tests.rs`

**Step 1: Write failing test**
Test creating a task and reading it back:
```rust
#[tokio::test]
async fn create_and_fetch_task() {
  let db = setup_test_db().await;
  let task = db.create_task("Refactor X", None).await.unwrap();
  let fetched = db.get_task(task.id).await.unwrap();
  assert_eq!(fetched.title, "Refactor X");
}
```
Expected: FAIL (db module missing)

**Step 2: Run test to verify it fails**
Run: `cargo test -p echo-tauri create_and_fetch_task`
Expected: FAIL

**Step 3: Write minimal schema + db code**
- Create SQL migration for tasks/agents/terminal_sessions/agent_logs/linear_issues
- Implement minimal DB wrapper with `create_task` + `get_task`

**Step 4: Run test to verify it passes**
Run: `cargo test -p echo-tauri create_and_fetch_task`
Expected: PASS

**Step 5: Commit**
```bash
git add src-tauri/migrations src-tauri/src/db

git commit -m "feat(db): add sqlite schema and task DAO"
```

### Task 4: Tauri commands + event bus skeleton

**Files:**
- Modify: `/Users/anshulmahajan/Desktop/Projects/echo/src/main.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/commands/mod.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/commands/tasks.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/commands/agents.rs`

**Step 1: Write failing test**
Use unit test on a command handler function (not the Tauri invocation) to ensure it creates a task.
Expected: FAIL

**Step 2: Run test to verify it fails**
Run: `cargo test -p echo-tauri create_task_command`
Expected: FAIL

**Step 3: Implement minimal command handlers**
- `create_task`, `update_task`, `delete_task`, `move_task_state`
- `create_agent`, `assign_agent_to_task`
- Emit `task_updated` and `agent_updated` events

**Step 4: Run test to verify it passes**
Run: `cargo test -p echo-tauri create_task_command`
Expected: PASS

**Step 5: Commit**
```bash
git add src-tauri/src/commands src-tauri/src/main.rs

git commit -m "feat(core): add tauri commands and event bus"
```

### Task 5: Terminal manager with PTY (TDD)

**Files:**
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/terminal/mod.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/terminal/tests.rs`

**Step 1: Write failing test**
Spawn a PTY and capture first output line.
Expected: FAIL

**Step 2: Run test to verify it fails**
Run: `cargo test -p echo-tauri pty_spawns`
Expected: FAIL

**Step 3: Implement minimal PTY manager**
- Spawn process, store session
- Read stdout and expose last_snippet

**Step 4: Run test to verify it passes**
Run: `cargo test -p echo-tauri pty_spawns`
Expected: PASS

**Step 5: Commit**
```bash
git add src-tauri/src/terminal

git commit -m "feat(terminal): add pty manager"
```

### Task 6: Frontend agent list + detail pane

**Files:**
- Modify: `/Users/anshulmahajan/Desktop/Projects/echo/.worktrees/echo-v1/src/routes/+page.svelte`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/.worktrees/echo-v1/src/lib/components/AgentList.svelte`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/.worktrees/echo-v1/src/lib/components/AgentDetail.svelte`

**Step 1: Write failing test (frontend)**
Add a component test verifying agent list renders name and state.
Expected: FAIL

**Step 2: Run test to verify it fails**
Run: `npm test -- AgentList`
Expected: FAIL

**Step 3: Implement minimal UI**
- Agent list with columns name/state/task/last update/snippet
- Detail pane for terminal output

**Step 4: Run test to verify it passes**
Run: `npm test -- AgentList`
Expected: PASS

**Step 5: Commit**
```bash
git add src/routes src/lib/components

git commit -m "feat(ui): add agent list and detail pane"
```

### Task 7: Voice pipeline stubs + intent parser wiring

**Files:**
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/voice/mod.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/voice/tests.rs`

**Step 1: Write failing test**
Test that an intent string is parsed into a command struct.
Expected: FAIL

**Step 2: Run test to verify it fails**
Run: `cargo test -p echo-tauri intent_parses`
Expected: FAIL

**Step 3: Implement minimal parser wrapper**
- Define JSON schema struct
- Stub Ollama call (return fixed JSON for now)

**Step 4: Run test to verify it passes**
Run: `cargo test -p echo-tauri intent_parses`
Expected: PASS

**Step 5: Commit**
```bash
git add src-tauri/src/voice

git commit -m "feat(voice): add intent parsing stub"
```

### Task 8: Linear importer (read-only)

**Files:**
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/linear/mod.rs`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/linear/tests.rs`

**Step 1: Write failing test**
Test that importer stores a Linear issue row.
Expected: FAIL

**Step 2: Run test to verify it fails**
Run: `cargo test -p echo-tauri linear_import`
Expected: FAIL

**Step 3: Implement minimal importer**
- HTTP client stub and DB insert

**Step 4: Run test to verify it passes**
Run: `cargo test -p echo-tauri linear_import`
Expected: PASS

**Step 5: Commit**
```bash
git add src-tauri/src/linear

git commit -m "feat(linear): add read-only import stub"
```

### Task 9: Wire commands to UI + event bus

**Files:**
- Modify: `/Users/anshulmahajan/Desktop/Projects/echo/.worktrees/echo-v1/src/routes/+page.svelte`
- Modify: `/Users/anshulmahajan/Desktop/Projects/echo/.worktrees/echo-v1/src/lib/components/AgentList.svelte`

**Step 1: Write failing test**
Simulate task creation and verify UI updates.
Expected: FAIL

**Step 2: Run test to verify it fails**
Run: `npm test -- TaskCreate`
Expected: FAIL

**Step 3: Implement minimal wiring**
- Use Tauri invoke for commands
- Subscribe to events for live updates

**Step 4: Run test to verify it passes**
Run: `npm test -- TaskCreate`
Expected: PASS

**Step 5: Commit**
```bash
git add src/routes src/lib/components

git commit -m "feat(ui): wire commands and events"
```

### Task 10: Add config handling and menu bar integration

**Files:**
- Modify: `/Users/anshulmahajan/Desktop/Projects/echo/tauri.conf.json`
- Create: `/Users/anshulmahajan/Desktop/Projects/echo/src/config/mod.rs`

**Step 1: Write failing test**
Config loader reads defaults and merges user config.
Expected: FAIL

**Step 2: Run test to verify it fails**
Run: `cargo test -p echo-tauri config_defaults`
Expected: FAIL

**Step 3: Implement minimal config loader**
- Read `~/.echo/config.toml`
- Provide defaults for mic + hotkeys + model endpoint

**Step 4: Run test to verify it passes**
Run: `cargo test -p echo-tauri config_defaults`
Expected: PASS

**Step 5: Commit**
```bash
git add src-tauri/src/config src-tauri/tauri.conf.json

git commit -m "feat(config): load config and menubar settings"
```

---