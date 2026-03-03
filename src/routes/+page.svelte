<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import AgentList from "$lib/components/AgentList.svelte";
  import AgentDetail from "$lib/components/AgentDetail.svelte";

  type Agent = {
    id: number;
    name: string;
    state: string;
    taskId?: number | null;
    lastSnippet?: string | null;
    updatedAt: string;
  };

  type Task = {
    id: number;
    title: string;
    state: string;
    updatedAt: string;
  };

  type ManagedSession = {
    id: number;
    provider: string;
    status: "waking" | "active" | "stalled" | "ended" | "failed";
    launchCommand: string;
    launchArgsJson: string;
    cwd?: string | null;
    pid?: number | null;
    agentId?: number | null;
    taskId?: number | null;
    lastHeartbeatAt?: string | null;
    startedAt?: string | null;
    endedAt?: string | null;
    failureReason?: string | null;
    createdAt: string;
    updatedAt: string;
  };

  type SessionEvent = {
    id: number;
    sessionId: number;
    eventType: string;
    message?: string | null;
    payloadJson?: string | null;
    createdAt: string;
  };

  type VoiceStatus = {
    running: boolean;
    state: string;
    lastTranscript?: string | null;
  };

  type ManagedSessionUpdatedEvent = {
    sessionId: number;
    status: ManagedSession["status"];
    lastHeartbeatAt?: string | null;
    agentId?: number | null;
    taskId?: number | null;
  };

  type ManagedSessionPromptRequiredEvent = {
    reason: "missing_command" | string;
    source: "voice" | "ui" | string;
  };

  type TerminalSnippetEvent = {
    agentId?: number | null;
    sessionId: number;
    snippet: string;
  };

  type VoiceIntentEvent = {
    action: string;
    payload: Record<string, unknown>;
  };

  type VoiceCommandExecutedEvent = {
    action: string;
    success: boolean;
    result: unknown;
  };

  type VoiceStatusReplyEvent = {
    text: string;
    query: string;
    resolved: Record<string, unknown>;
  };

  let agents = $state<Agent[]>([]);
  let tasks = $state<Task[]>([]);
  let sessions = $state<ManagedSession[]>([]);
  let selectedAgentId = $state<number>(0);
  let loading = $state<boolean>(true);
  let voiceRunning = $state<boolean>(false);
  let voiceState = $state<string>("idle");
  let lastTranscript = $state<string>("");
  let lastIntent = $state<string>("");
  let lastCommand = $state<string>("");
  let voiceInput = $state<string>("");
  let sessionEvents = $state<SessionEvent[]>([]);
  let sessionEventsFor = $state<number | null>(null);
  let selectedSessionId = $state<number | null>(null);
  let liveTerminalOutput = $state<string>("");
  let terminalInput = $state<string>("");

  const selectedAgent = $derived(
    agents.find((agent) => agent.id === selectedAgentId)
  );

  const selectedTask = $derived(
    tasks.find((task) => task.id === selectedAgent?.taskId)
  );

  const selectedAgentSession = $derived(
    selectedAgent
      ? sessions.find(
          (session) =>
            session.agentId === selectedAgent.id &&
            ["waking", "active", "stalled"].includes(session.status)
        )
      : undefined
  );

  const selectedSession = $derived(
    selectedSessionId ? sessions.find((session) => session.id === selectedSessionId) : undefined
  );

  const formatRelativeTime = (value: string | null | undefined) => {
    if (!value) return "-";
    const normalized = value.includes("T") ? value : value.replace(" ", "T") + "Z";
    const date = new Date(normalized);
    if (Number.isNaN(date.getTime())) return value;
    const diffSeconds = Math.max(0, Math.floor((Date.now() - date.getTime()) / 1000));
    if (diffSeconds < 60) return "just now";
    const minutes = Math.floor(diffSeconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    return `${days}d ago`;
  };

  const applyRelativeTimes = (list: Agent[], taskList: Task[]) => {
    agents = list.map((agent) => ({
      ...agent,
      updatedAt: formatRelativeTime(agent.updatedAt)
    }));
    tasks = taskList.map((task) => ({
      ...task,
      updatedAt: formatRelativeTime(task.updatedAt)
    }));
  };

  async function loadData() {
    loading = true;
    try {
      const [tasksResponse, agentsResponse, sessionsResponse] = await Promise.all([
        invoke("list_tasks_cmd"),
        invoke("list_agents_cmd"),
        invoke("list_managed_sessions_cmd", { status: null, limit: 100 })
      ]);
      applyRelativeTimes(agentsResponse as Agent[], tasksResponse as Task[]);
      sessions = sessionsResponse as ManagedSession[];
      if (!selectedSessionId && sessions.length > 0) {
        selectedSessionId = sessions[0].id;
      }
      if (selectedSessionId && !sessions.some((session) => session.id === selectedSessionId)) {
        selectedSessionId = sessions.length > 0 ? sessions[0].id : null;
      }
      if (selectedSessionId) {
        void loadTerminalOutput(selectedSessionId);
      } else {
        liveTerminalOutput = "";
      }
      if (agents.length > 0 && !agents.some((agent) => agent.id === selectedAgentId)) {
        selectedAgentId = agents[0].id;
      }
      if (agents.length === 0) {
        selectedAgentId = 0;
      }
    } catch (error) {
      console.error("Failed to load data", error);
    } finally {
      loading = false;
    }
  }

  async function startSessionPrompt(agentId?: number, taskId?: number, initial?: string) {
    const value = window.prompt("Command to start session", initial ?? "opencode");
    const command = value?.trim();
    if (!command) return;
    try {
      await invoke("start_managed_session_cmd", {
        request: {
          command,
          args: [],
          cwd: null,
          agentId: agentId ?? null,
          taskId: taskId ?? null,
          provider: "opencode"
        }
      });
      await loadData();
    } catch (error) {
      console.error("Failed to start managed session", error);
    }
  }

  async function openSession() {
    if (!selectedAgent) return;
    await startSessionPrompt(selectedAgent.id, selectedAgent.taskId ?? undefined);
  }

  async function stopSession(sessionId: number) {
    try {
      await invoke("stop_managed_session_cmd", { sessionId });
      await loadData();
    } catch (error) {
      console.error("Failed to stop session", error);
    }
  }

  async function restartSession(session: ManagedSession) {
    await stopSession(session.id);
    await startSessionPrompt(session.agentId ?? undefined, session.taskId ?? undefined, session.launchCommand);
  }

  async function openSessionLogs(sessionId: number) {
    try {
      const response = (await invoke("list_session_events_cmd", {
        sessionId,
        limit: 50
      })) as SessionEvent[];
      sessionEventsFor = sessionId;
      sessionEvents = response;
    } catch (error) {
      console.error("Failed to load session events", error);
    }
  }

  async function loadTerminalOutput(sessionId: number) {
    try {
      const output = (await invoke("get_terminal_output_cmd", {
        sessionId
      })) as string;
      liveTerminalOutput = output || "";
    } catch (error) {
      console.error("Failed to load terminal output", error);
    }
  }

  async function sendTerminalInput() {
    const text = terminalInput;
    if (!selectedSessionId || !text.trim()) return;
    const payload = text.endsWith("\n") ? text : `${text}\n`;
    try {
      await invoke("send_terminal_input_cmd", {
        sessionId: selectedSessionId,
        input: payload
      });
      terminalInput = "";
    } catch (error) {
      console.error("Failed to send terminal input", error);
    }
  }

  async function createTask() {
    const title = `Untitled task ${tasks.length + 1}`;
    try {
      await invoke("create_task_cmd", { title });
      await loadData();
    } catch (error) {
      console.error("Failed to create task", error);
    }
  }

  async function createAgent() {
    const name = `Agent ${agents.length + 1}`;
    try {
      const agent = (await invoke("create_agent_cmd", { name })) as Agent;
      await loadData();
      selectedAgentId = agent.id;
    } catch (error) {
      console.error("Failed to create agent", error);
    }
  }

  async function startVoice() {
    try {
      const status = (await invoke("start_voice_cmd")) as VoiceStatus;
      voiceRunning = status.running;
      voiceState = status.state;
    } catch (error) {
      console.error("Failed to start voice pipeline", error);
    }
  }

  async function stopVoice() {
    try {
      const status = (await invoke("stop_voice_cmd")) as VoiceStatus;
      voiceRunning = status.running;
      voiceState = status.state;
    } catch (error) {
      console.error("Failed to stop voice pipeline", error);
    }
  }

  async function submitVoiceText() {
    const text = voiceInput.trim();
    if (!text) return;
    try {
      await invoke("process_voice_text_cmd", { text });
      voiceInput = "";
    } catch (error) {
      console.error("Failed to process voice text", error);
    }
  }

  onMount(() => {
    let unlistenTask: (() => void) | undefined;
    let unlistenAgent: (() => void) | undefined;
    let unlistenTerminal: (() => void) | undefined;
    let unlistenSession: (() => void) | undefined;
    let unlistenSessionPrompt: (() => void) | undefined;
    let unlistenVoiceState: (() => void) | undefined;
    let unlistenVoiceTranscript: (() => void) | undefined;
    let unlistenVoiceIntent: (() => void) | undefined;
    let unlistenVoiceCommand: (() => void) | undefined;
    let unlistenVoiceError: (() => void) | undefined;
    let unlistenVoiceReply: (() => void) | undefined;

    const startListeners = async () => {
      unlistenTask = await listen("task_updated", () => {
        loadData();
      });
      unlistenAgent = await listen("agent_updated", () => {
        loadData();
      });
      unlistenTerminal = await listen("terminal_snippet_updated", (event) => {
        const payload = event.payload as TerminalSnippetEvent;
        if (payload.agentId) {
          agents = agents.map((agent) =>
            agent.id === payload.agentId
              ? { ...agent, lastSnippet: payload.snippet, updatedAt: "just now" }
              : agent
          );
        }
        if (selectedSessionId === payload.sessionId) {
          liveTerminalOutput = `${liveTerminalOutput}${payload.snippet}`.slice(-500000);
        }
      });
      unlistenSession = await listen("managed_session_updated", (event) => {
        const payload = event.payload as ManagedSessionUpdatedEvent;
        sessions = sessions.map((session) =>
          session.id === payload.sessionId
            ? {
                ...session,
                status: payload.status,
                lastHeartbeatAt: payload.lastHeartbeatAt ?? session.lastHeartbeatAt
              }
            : session
        );
        void loadData();
      });
      unlistenSessionPrompt = await listen("managed_session_prompt_required", async (event) => {
        const payload = event.payload as ManagedSessionPromptRequiredEvent;
        if (payload.reason === "missing_command") {
          await startSessionPrompt();
        }
      });
      unlistenVoiceState = await listen("voice_state_updated", (event) => {
        const payload = event.payload as { state: string };
        voiceState = payload.state;
      });
      unlistenVoiceTranscript = await listen("voice_transcript", (event) => {
        const payload = event.payload as { text: string };
        lastTranscript = payload.text;
      });
      unlistenVoiceIntent = await listen("voice_intent", (event) => {
        const payload = event.payload as VoiceIntentEvent;
        lastIntent = `${payload.action} ${JSON.stringify(payload.payload)}`;
      });
      unlistenVoiceCommand = await listen("voice_command_executed", (event) => {
        const payload = event.payload as VoiceCommandExecutedEvent;
        lastCommand = `${payload.action} (${payload.success ? "ok" : "failed"})`;
        loadData();
      });
      unlistenVoiceError = await listen("voice_error", (event) => {
        const payload = event.payload as { message: string };
        lastCommand = `error: ${payload.message}`;
      });
      unlistenVoiceReply = await listen("voice_status_reply", (event) => {
        const payload = event.payload as VoiceStatusReplyEvent;
        lastCommand = payload.text;
      });
    };

    startListeners();
    invoke("voice_status_cmd")
      .then((status) => {
        const typed = status as VoiceStatus;
        voiceRunning = typed.running;
        voiceState = typed.state;
        lastTranscript = typed.lastTranscript ?? "";
      })
      .catch((error) => {
        console.error("Failed to get voice status", error);
      });

    const interval = setInterval(loadData, 8000);
    loadData();
    const outputInterval = setInterval(() => {
      if (selectedSessionId) {
        void loadTerminalOutput(selectedSessionId);
      }
    }, 5000);

    return () => {
      clearInterval(interval);
      clearInterval(outputInterval);
      unlistenTask?.();
      unlistenAgent?.();
      unlistenTerminal?.();
      unlistenSession?.();
      unlistenSessionPrompt?.();
      unlistenVoiceState?.();
      unlistenVoiceTranscript?.();
      unlistenVoiceIntent?.();
      unlistenVoiceCommand?.();
      unlistenVoiceError?.();
      unlistenVoiceReply?.();
    };
  });
</script>

<main class="app">
  <header class="app-header">
    <div>
      <p class="eyebrow">Echo Orchestrator</p>
      <h1>Voice-first agent console</h1>
      <p class="subhead">
        Coordinate agents, attach tasks, and stream terminal output in a single
        canvas.
      </p>
      <p class="voice-status">Voice: {voiceState}</p>
    </div>
    <div class="header-actions">
      <button class="ghost" onclick={createTask}>New task</button>
      <button class="primary" onclick={createAgent}>Spawn agent</button>
      <button class="ghost" onclick={() => startSessionPrompt()}>Start session</button>
      {#if voiceRunning}
        <button class="ghost" onclick={stopVoice}>Stop voice</button>
      {:else}
        <button class="ghost" onclick={startVoice}>Start voice</button>
      {/if}
    </div>
  </header>

  <section class="voice-panel">
    <input
      bind:value={voiceInput}
      placeholder="Test transcript (bridge until mic capture is wired)"
      onkeydown={(event) => event.key === "Enter" && submitVoiceText()}
    />
    <button class="primary" onclick={submitVoiceText}>Run voice command</button>
    <p>Transcript: {lastTranscript || "none"}</p>
    <p>Intent: {lastIntent || "none"}</p>
    <p>Command: {lastCommand || "none"}</p>
  </section>

  <section class="layout">
    <AgentList
      {agents}
      {tasks}
      selectedId={selectedAgentId}
      onSelect={(id: number) => (selectedAgentId = id)}
    />
    <AgentDetail
      agent={selectedAgent}
      task={selectedTask}
      onOpenSession={openSession}
      canOpenSession={!!selectedAgent}
      linkedSession={
        selectedAgentSession
          ? {
              id: selectedAgentSession.id,
              status: selectedAgentSession.status,
              launchCommand: selectedAgentSession.launchCommand
            }
          : undefined
      }
    />
  </section>

  <section class="sessions-panel">
    <header>
      <h2>Managed sessions</h2>
      <span>{sessions.length} total</span>
    </header>
    <div class="sessions-table">
      <div class="row header">
        <span>ID</span>
        <span>Status</span>
        <span>Command</span>
        <span>Agent</span>
        <span>Task</span>
        <span>Heartbeat</span>
        <span>Actions</span>
      </div>
      {#if sessions.length === 0}
        <div class="row empty"><span>No sessions yet</span></div>
      {:else}
        {#each sessions as session}
          <div class="row">
            <span>#{session.id}</span>
            <span class={`status ${session.status}`}>{session.status}</span>
            <span>{session.launchCommand}</span>
            <span>{session.agentId ?? "-"}</span>
            <span>{session.taskId ?? "-"}</span>
            <span>{formatRelativeTime(session.lastHeartbeatAt ?? session.updatedAt)}</span>
            <span class="actions">
              <button
                class="ghost small"
                onclick={() => {
                  selectedSessionId = session.id;
                  void loadTerminalOutput(session.id);
                }}
              >
                Open terminal
              </button>
              <button class="ghost small" onclick={() => openSessionLogs(session.id)}>Open logs</button>
              {#if session.status === "active" || session.status === "stalled" || session.status === "waking"}
                <button class="ghost small" onclick={() => stopSession(session.id)}>Stop</button>
              {/if}
              <button class="primary small" onclick={() => restartSession(session)}>Restart</button>
            </span>
          </div>
        {/each}
      {/if}
    </div>

    {#if sessionEventsFor}
      <div class="logs">
        <h3>Session #{sessionEventsFor} logs</h3>
        {#if sessionEvents.length === 0}
          <p>No events.</p>
        {:else}
          {#each sessionEvents as event}
            <p>
              <strong>{event.eventType}</strong> [{formatRelativeTime(event.createdAt)}]
              {event.message ?? ""}
            </p>
          {/each}
        {/if}
      </div>
    {/if}
  </section>

  <section class="terminal-panel">
    <header>
      <h2>Live terminal</h2>
      <span>{selectedSession ? `Session #${selectedSession.id}` : "No session selected"}</span>
    </header>
    <pre>{liveTerminalOutput || "No terminal output yet."}</pre>
    <div class="terminal-input-row">
      <input
        bind:value={terminalInput}
        placeholder="Type a command for the selected session"
        onkeydown={(event) => event.key === "Enter" && sendTerminalInput()}
        disabled={!selectedSessionId}
      />
      <button class="primary" onclick={sendTerminalInput} disabled={!selectedSessionId}>Send</button>
    </div>
  </section>

  {#if loading}
    <p class="loading">Loading...</p>
  {/if}
</main>

<style>
  :global(body) {
    margin: 0;
    background: radial-gradient(circle at top, #1f242c 0%, #0d0f12 60%);
    color: #f4f2ee;
    font-family: "Space Grotesk", "Avenir Next", "Helvetica Neue", sans-serif;
  }

  .app {
    min-height: 100vh;
    padding: 48px 56px;
    background-image: linear-gradient(120deg, rgba(56, 86, 120, 0.2), transparent),
      radial-gradient(circle at 20% 20%, rgba(255, 173, 93, 0.18), transparent 45%),
      radial-gradient(circle at 80% 0%, rgba(63, 155, 255, 0.12), transparent 40%);
  }

  .app-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 32px;
    margin-bottom: 32px;
  }

  .eyebrow {
    text-transform: uppercase;
    letter-spacing: 0.24em;
    font-size: 11px;
    color: rgba(244, 242, 238, 0.6);
    margin: 0 0 6px;
  }

  h1 {
    font-size: 36px;
    margin: 0 0 8px;
  }

  .subhead {
    margin: 0;
    max-width: 480px;
    color: rgba(244, 242, 238, 0.7);
  }

  .voice-status {
    margin-top: 10px;
    color: rgba(244, 242, 238, 0.78);
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .header-actions {
    display: flex;
    gap: 12px;
  }

  button {
    border: none;
    padding: 10px 18px;
    border-radius: 999px;
    font-weight: 600;
    cursor: pointer;
  }

  .small {
    padding: 6px 10px;
    font-size: 12px;
  }

  .ghost {
    background: rgba(255, 255, 255, 0.08);
    color: #f4f2ee;
  }

  .primary {
    background: linear-gradient(120deg, #ff9f43, #ff6b6b);
    color: #120c08;
  }

  .layout {
    display: grid;
    grid-template-columns: minmax(280px, 1.2fr) minmax(320px, 1fr);
    gap: 24px;
  }

  .voice-panel {
    margin-bottom: 18px;
    padding: 14px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.08);
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 10px;
    align-items: center;
  }

  .voice-panel input {
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.18);
    background: rgba(8, 10, 14, 0.7);
    color: #f4f2ee;
  }

  .voice-panel p {
    margin: 0;
    grid-column: 1 / -1;
    color: rgba(244, 242, 238, 0.78);
    font-size: 13px;
  }

  .sessions-panel {
    margin-top: 20px;
    padding: 16px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .sessions-panel header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .sessions-table {
    display: grid;
    gap: 6px;
  }

  .row {
    display: grid;
    grid-template-columns: 0.5fr 0.8fr 1.3fr 0.6fr 0.6fr 0.9fr 1.4fr;
    gap: 10px;
    align-items: center;
    padding: 10px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.03);
  }

  .row.header {
    font-size: 11px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: rgba(244, 242, 238, 0.6);
    background: transparent;
  }

  .row.empty {
    grid-template-columns: 1fr;
  }

  .status.active {
    color: #7bdff2;
  }

  .status.stalled,
  .status.failed {
    color: #ef476f;
  }

  .status.waking {
    color: #ffd166;
  }

  .status.ended {
    color: rgba(244, 242, 238, 0.5);
  }

  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .logs {
    margin-top: 12px;
    border-top: 1px solid rgba(255, 255, 255, 0.1);
    padding-top: 10px;
  }

  .logs p {
    margin: 6px 0;
    font-size: 13px;
    color: rgba(244, 242, 238, 0.78);
  }

  .terminal-panel {
    margin-top: 20px;
    padding: 16px;
    border-radius: 16px;
    background: rgba(9, 11, 14, 0.85);
    border: 1px solid rgba(255, 255, 255, 0.1);
  }

  .terminal-panel header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .terminal-panel pre {
    margin: 0 0 12px;
    height: 260px;
    overflow: auto;
    background: #07090d;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
    padding: 12px;
    color: rgba(244, 242, 238, 0.9);
    font-family: "JetBrains Mono", "Menlo", monospace;
    font-size: 12px;
    line-height: 1.45;
    white-space: pre-wrap;
  }

  .terminal-input-row {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 10px;
  }

  .terminal-input-row input {
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid rgba(255, 255, 255, 0.18);
    background: rgba(8, 10, 14, 0.7);
    color: #f4f2ee;
  }

  .loading {
    margin-top: 14px;
    color: rgba(244, 242, 238, 0.68);
  }

  @media (max-width: 980px) {
    .app-header {
      flex-direction: column;
      align-items: flex-start;
    }

    .layout {
      grid-template-columns: 1fr;
    }

    .row {
      grid-template-columns: 1fr;
      gap: 6px;
    }
  }
</style>
