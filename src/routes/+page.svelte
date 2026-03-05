<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import AgentList from "$lib/components/AgentList.svelte";
  import AgentDetail from "$lib/components/AgentDetail.svelte";

  type AgentRow = {
    id: number;
    name: string;
    state: string;
    provider: string;
    displayOrder: number;
    attentionState: string;
    taskId?: number | null;
    taskTitle?: string | null;
    activeSessionId?: number | null;
    activeSessionStatus?: string | null;
    activeSessionNeedsInput?: boolean | null;
    activeSessionInputReason?: string | null;
    unresolvedAlertCount: number;
    lastActivityAt?: string | null;
    lastInputRequiredAt?: string | null;
    lastSnippet?: string | null;
    updatedAt: string;
  };

  type AgentRowPayload = {
    agentId: number;
    agentName: string;
    agentState: string;
    provider: string;
    displayOrder: number;
    attentionState: string;
    taskId?: number | null;
    taskTitle?: string | null;
    activeSessionId?: number | null;
    activeSessionStatus?: string | null;
    activeSessionNeedsInput?: boolean | null;
    activeSessionInputReason?: string | null;
    lastActivityAt?: string | null;
    lastSnippet?: string | null;
    unresolvedAlertCount: number;
    updatedAt: string;
  };

  type Task = {
    id: number;
    title: string;
    state: string;
    updatedAt: string;
  };

  type SessionRuntime = {
    id: number;
    provider: string;
    status: "waking" | "active" | "stalled" | "needs_input" | "ended" | "failed";
    launchCommand: string;
    launchArgsJson: string;
    cwd?: string | null;
    pid?: number | null;
    agentId?: number | null;
    taskId?: number | null;
    lastHeartbeatAt?: string | null;
    startedAt?: string | null;
    endedAt?: string | null;
    needsInput?: boolean;
    inputReason?: string | null;
    lastActivityAt?: string | null;
    transport?: string;
    attachCount?: number;
    failureReason?: string | null;
    createdAt: string;
    updatedAt: string;
  };

  type SessionAlert = {
    id: number;
    sessionId: number;
    agentId?: number | null;
    severity: "info" | "warning" | "critical" | string;
    reason: string;
    message: string;
    requiresAck: boolean;
    acknowledgedAt?: string | null;
    resolvedAt?: string | null;
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

  type AgentRuntimeUpdatedEvent = {
    agentId: number;
    activeSessionId?: number | null;
    status: SessionRuntime["status"];
    attentionState: string;
    lastActivityAt?: string | null;
  };

  type ManagedSessionPromptRequiredEvent = {
    reason: "missing_command" | string;
    source: "voice" | "ui" | string;
    action?: string;
    message?: string;
  };

  type TerminalChunkEvent = {
    sessionId: number;
    chunk: string;
    cursor: number;
    isDelta: boolean;
    at: string;
  };

  type TerminalOutputChunk = {
    sessionId: number;
    chunk: string;
    cursor: number;
    hasMore: boolean;
    isDelta: boolean;
    at: string;
  };

  type VoiceIntentEvent = {
    action: string;
    payload: Record<string, unknown>;
  };

  type VoiceAction = {
    action: string;
    targetAgentId?: number | null;
    targetSessionId?: number | null;
    text?: string | null;
    result: string;
    at: string;
  };

  type VoiceStatusReplyEvent = {
    requestType: string;
    targetAgentId?: number | null;
    summary: string;
    at: string;
  };

  type TerminalViewportState = {
    selectedSessionId: number | null;
    cursors: Map<number, number>;
  };

  let agents = $state<AgentRow[]>([]);
  let tasks = $state<Task[]>([]);
  let sessions = $state<SessionRuntime[]>([]);
  let selectedAgentId = $state<number>(0);
  let loading = $state<boolean>(true);
  let voiceRunning = $state<boolean>(false);
  let voiceState = $state<string>("idle");
  let lastTranscript = $state<string>("");
  let lastIntent = $state<string>("");
  let lastCommand = $state<string>("");
  let voiceInput = $state<string>("");
  let lastVoiceCommandText = $state<string>("");
  let pushToTalkBusy = $state<boolean>(false);
  let sessionEvents = $state<SessionEvent[]>([]);
  let unresolvedAlerts = $state<SessionAlert[]>([]);
  let sessionEventsFor = $state<number | null>(null);
  let selectedSessionId = $state<number | null>(null);
  let terminalInput = $state<string>("");
  let terminalContainer: HTMLDivElement | null = $state(null);

  let terminalWidget: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  const terminalViewportState: TerminalViewportState = {
    selectedSessionId: null,
    cursors: new Map<number, number>()
  };
  const terminalCursorBySession = terminalViewportState.cursors;
  const TERMINAL_CHUNK_BYTES = 16_384;
  const TERMINAL_MAX_DRAIN_ITERATIONS = 8;

  const selectedAgent = $derived(
    agents.find((agent) => agent.id === selectedAgentId)
  );

  const selectedTask = $derived(
    tasks.find((task) => task.id === selectedAgent?.taskId)
  );

  const selectedAgentAlerts = $derived(
    selectedAgent
      ? unresolvedAlerts.filter((alert) => alert.agentId === selectedAgent.id)
      : []
  );

  const selectedAgentSession = $derived(
    selectedAgent
      ? sessions.find((session) => session.id === selectedAgent.activeSessionId) ??
        sessions.find(
          (session) =>
            session.agentId === selectedAgent.id &&
            ["waking", "active", "stalled"].includes(session.status)
        )
      : undefined
  );

  const selectedSession = $derived(
    selectedSessionId ? sessions.find((session) => session.id === selectedSessionId) : undefined
  );

  const pendingAckAgentIds = $derived(
    Array.from(
      new Set(
        unresolvedAlerts
          .filter((alert) => alert.requiresAck && !alert.acknowledgedAt && alert.agentId)
          .map((alert) => alert.agentId as number)
      )
    )
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

  const applyRelativeTimes = (
    list: AgentRow[],
    taskList: Task[],
    alertList: SessionAlert[]
  ) => {
    agents = list.map((agent) => ({
      ...agent,
      updatedAt: formatRelativeTime(agent.updatedAt),
      lastActivityAt: formatRelativeTime(agent.lastActivityAt ?? agent.updatedAt),
      lastInputRequiredAt: formatRelativeTime(agent.lastInputRequiredAt ?? null)
    }));
    tasks = taskList.map((task) => ({
      ...task,
      updatedAt: formatRelativeTime(task.updatedAt)
    }));
    unresolvedAlerts = alertList.map((alert) => ({
      ...alert,
      createdAt: formatRelativeTime(alert.createdAt),
      updatedAt: formatRelativeTime(alert.updatedAt),
      acknowledgedAt: alert.acknowledgedAt ? formatRelativeTime(alert.acknowledgedAt) : null,
      resolvedAt: alert.resolvedAt ? formatRelativeTime(alert.resolvedAt) : null
    }));
  };

  async function loadData() {
    loading = true;
    try {
      const [tasksResponse, agentRowsResponse, sessionsResponse, alertsResponse] = await Promise.all([
        invoke("list_tasks_cmd"),
        invoke("list_agent_rows_cmd", { limit: 200 }),
        invoke("list_managed_sessions_cmd", { status: null, limit: 200 }),
        invoke("list_session_alerts_cmd", {
          agentId: null,
          unresolvedOnly: true,
          limit: 200
        })
      ]);
      const mappedAgents = (agentRowsResponse as AgentRowPayload[]).map((row) => ({
        id: row.agentId,
        name: row.agentName,
        state: row.agentState,
        provider: row.provider,
        displayOrder: row.displayOrder,
        attentionState: row.attentionState,
        taskId: row.taskId ?? null,
        taskTitle: row.taskTitle ?? null,
        activeSessionId: row.activeSessionId ?? null,
        activeSessionStatus: row.activeSessionStatus ?? null,
        activeSessionNeedsInput: row.activeSessionNeedsInput ?? null,
        activeSessionInputReason: row.activeSessionInputReason ?? null,
        unresolvedAlertCount: row.unresolvedAlertCount ?? 0,
        lastActivityAt: row.lastActivityAt ?? null,
        lastSnippet: row.lastSnippet ?? null,
        updatedAt: row.updatedAt
      }));
      applyRelativeTimes(
        mappedAgents,
        tasksResponse as Task[],
        alertsResponse as SessionAlert[]
      );
      sessions = sessionsResponse as SessionRuntime[];
      if (!selectedSessionId && sessions.length > 0) {
        selectedSessionId = sessions[0].id;
      }
      if (selectedSessionId && !sessions.some((session) => session.id === selectedSessionId)) {
        selectedSessionId = sessions.length > 0 ? sessions[0].id : null;
      }
      if (selectedSessionId) {
        void hydrateTerminalSession(selectedSessionId, { reset: true });
      } else {
        clearTerminalView();
      }
      if (agents.length > 0 && !agents.some((agent) => agent.id === selectedAgentId)) {
        selectedAgentId = agents[0].id;
      }
      if (agents.length === 0) {
        selectedAgentId = 0;
      }
      const focusedAgent = agents.find((agent) => agent.id === selectedAgentId);
      if (focusedAgent?.activeSessionId) {
        selectedSessionId = focusedAgent.activeSessionId;
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
    if (!agentId) return;
    try {
      await invoke("start_agent_session_cmd", {
        agentId,
        launchProfile: {
          command,
          args: [],
          cwd: null,
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
      await invoke("stop_agent_session_cmd", { sessionId });
      await loadData();
    } catch (error) {
      console.error("Failed to stop session", error);
    }
  }

  async function restartSession(session: SessionRuntime) {
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

  const lookupAgentName = (agentId?: number | null) =>
    agents.find((agent) => agent.id === agentId)?.name ?? "Unassigned";

  const findSessionForAgent = (agentId: number) =>
    sessions.find((session) => session.id === agents.find((agent) => agent.id === agentId)?.activeSessionId) ??
    sessions.find(
      (session) =>
        session.agentId === agentId &&
        ["waking", "active", "stalled", "needs_input"].includes(session.status)
    );

  const focusAgentById = (agentId: number, preferredSessionId?: number | null) => {
    const target = agents.find((agent) => agent.id === agentId);
    if (!target) return;
    selectedAgentId = target.id;
    if (preferredSessionId) {
      selectedSessionId = preferredSessionId;
      void hydrateTerminalSession(preferredSessionId, { reset: true });
      return;
    }
    const fallbackSession = findSessionForAgent(target.id);
    if (fallbackSession) {
      selectedSessionId = fallbackSession.id;
      void hydrateTerminalSession(fallbackSession.id, { reset: true });
    }
  };

  function focusSession(sessionId: number) {
    selectedSessionId = sessionId;
    const session = sessions.find((entry) => entry.id === sessionId);
    if (session?.agentId) {
      selectedAgentId = session.agentId;
    }
    void hydrateTerminalSession(sessionId, { reset: true });
  }

  async function attachFromAgent(agentId: number) {
    focusAgentById(agentId);
    const active = findSessionForAgent(agentId);
    if (active) {
      focusSession(active.id);
      return;
    }
    const agent = agents.find((entry) => entry.id === agentId);
    await startSessionPrompt(agentId, agent?.taskId ?? undefined);
  }

  async function replyFromAgent(agentId: number) {
    focusAgentById(agentId);
    const active = findSessionForAgent(agentId);
    if (!active) return;
    const text = window.prompt(`Reply to ${agents.find((entry) => entry.id === agentId)?.name ?? "agent"}`);
    const payload = text?.trim();
    if (!payload) return;
    await invoke("send_terminal_input_cmd", {
      sessionId: active.id,
      input: payload.endsWith("\n") ? payload : `${payload}\n`
    });
    if (selectedSessionId === active.id) {
      await streamTerminalChunks(active.id);
    }
  }

  async function acknowledgeFromAgent(agentId: number) {
    const alert = unresolvedAlerts.find(
      (entry) =>
        entry.agentId === agentId && entry.requiresAck && !entry.acknowledgedAt && !entry.resolvedAt
    );
    if (!alert) return;
    await acknowledgeAlert(alert.id);
    focusAgentById(agentId, alert.sessionId);
  }

  function handleGlobalAgentNavigation(event: KeyboardEvent) {
    const target = event.target as HTMLElement | null;
    if (!target) return;
    const tagName = target.tagName.toLowerCase();
    if (tagName === "input" || tagName === "textarea" || target.isContentEditable) return;
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    if (agents.length === 0) return;
    event.preventDefault();
    const currentIndex = Math.max(0, agents.findIndex((agent) => agent.id === selectedAgentId));
    const nextIndex =
      event.key === "ArrowDown"
        ? Math.min(agents.length - 1, currentIndex + 1)
        : Math.max(0, currentIndex - 1);
    focusAgentById(agents[nextIndex].id);
  }

  async function acknowledgeAlert(alertId: number) {
    try {
      await invoke("acknowledge_session_alert_cmd", { alertId });
      await loadData();
    } catch (error) {
      console.error("Failed to acknowledge session alert", error);
    }
  }

  async function resolveAlert(alertId: number) {
    try {
      await invoke("resolve_session_alert_cmd", { alertId });
      await loadData();
    } catch (error) {
      console.error("Failed to resolve session alert", error);
    }
  }

  function clearTerminalView() {
    terminalWidget?.clear();
    terminalWidget?.reset();
    if (terminalWidget) {
      terminalWidget.write("No terminal output yet.\r\n");
    }
  }

  async function streamTerminalChunks(sessionId: number) {
    let cursor = terminalCursorBySession.get(sessionId) ?? 0;
    try {
      for (let i = 0; i < TERMINAL_MAX_DRAIN_ITERATIONS; i += 1) {
        const payload = (await invoke("get_terminal_output_cmd", {
          sessionId,
          cursor,
          maxBytes: TERMINAL_CHUNK_BYTES
        })) as TerminalOutputChunk;
        if (!payload.chunk) {
          cursor = payload.cursor;
          break;
        }
        cursor = payload.cursor;
        if (selectedSessionId === sessionId && terminalWidget) {
          terminalWidget.write(payload.chunk);
        }
        if (!payload.hasMore) {
          break;
        }
      }
      terminalCursorBySession.set(sessionId, cursor);
    } catch (error) {
      console.error("Failed to stream terminal output", error);
    }
  }

  async function hydrateTerminalSession(sessionId: number, options: { reset?: boolean } = {}) {
    const { reset = false } = options;
    if (reset) {
      terminalWidget?.clear();
      terminalWidget?.reset();
      terminalCursorBySession.set(sessionId, 0);
    }
    await streamTerminalChunks(sessionId);
    await resizeTerminalSession(sessionId);
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
      await streamTerminalChunks(selectedSessionId);
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
      const agent = (await invoke("create_agent_cmd", { name })) as { id: number };
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

  async function runVoiceText(
    text: string,
    options: { pushToTalk?: boolean; confirmed?: boolean } = {}
  ) {
    const { pushToTalk = false, confirmed = false } = options;
    const transcript = confirmed ? `confirm ${text}` : text;
    let startedTemporarily = false;
    if (pushToTalk && !voiceRunning) {
      const status = (await invoke("start_voice_cmd")) as VoiceStatus;
      voiceRunning = status.running;
      voiceState = status.state;
      startedTemporarily = true;
    }
    if (!text) return;
    try {
      if (pushToTalk) {
        pushToTalkBusy = true;
      }
      await invoke("process_voice_text_cmd", { text: transcript });
    } catch (error) {
      console.error("Failed to process voice text", error);
    } finally {
      if (startedTemporarily) {
        const status = (await invoke("stop_voice_cmd")) as VoiceStatus;
        voiceRunning = status.running;
        voiceState = status.state;
      }
      if (pushToTalk) {
        pushToTalkBusy = false;
      }
    }
  }

  async function submitVoiceText() {
    const text = voiceInput.trim();
    if (!text) return;
    await runVoiceText(text);
    voiceInput = "";
  }

  async function pushToTalkVoiceText() {
    if (pushToTalkBusy) return;
    pushToTalkBusy = true;
    try {
      await invoke("push_to_talk_cmd");
    } catch (error) {
      console.error("Failed to run push-to-talk command", error);
    } finally {
      pushToTalkBusy = false;
      invoke("voice_status_cmd")
        .then((status) => {
          const typed = status as VoiceStatus;
          voiceRunning = typed.running;
          voiceState = typed.state;
          lastTranscript = typed.lastTranscript ?? lastTranscript;
        })
        .catch((error) => {
          console.error("Failed to refresh voice status", error);
        });
    }
  }

  async function initTerminalWidget() {
    if (!terminalContainer) return;
    try {
      fitAddon = new FitAddon();
      terminalWidget = new Terminal({
        cursorBlink: true,
        convertEol: true,
        fontFamily: '"JetBrains Mono", "Menlo", monospace',
        fontSize: 12,
        lineHeight: 1.35,
        theme: {
          background: "#07090d",
          foreground: "#f4f2ee",
          cursor: "#ff9f43"
        }
      });
      terminalWidget.loadAddon(fitAddon);
      terminalWidget.open(terminalContainer);
      fitAddon.fit();
      clearTerminalView();
      if (selectedSessionId) {
        await resizeTerminalSession(selectedSessionId);
      }
    } catch (error) {
      console.error("Failed to initialize terminal widget", error);
      terminalContainer.textContent =
        "Terminal widget unavailable. Install xterm dependencies to enable attach.";
    }
  }

  async function resizeTerminalSession(sessionId: number) {
    if (!terminalWidget) return;
    const cols = terminalWidget.cols;
    const rows = terminalWidget.rows;
    if (cols < 2 || rows < 2) return;
    try {
      await invoke("resize_terminal_cmd", { sessionId, cols, rows });
    } catch (error) {
      console.error("Failed to resize terminal session", error);
    }
  }

  $effect(() => {
    if (!selectedSessionId) {
      sessionEventsFor = null;
      sessionEvents = [];
      return;
    }
    void openSessionLogs(selectedSessionId);
  });

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
    let unlistenAlertResolved: (() => void) | undefined;

    void initTerminalWidget();

    const startListeners = async () => {
      unlistenTask = await listen("task_updated", () => {
        loadData();
      });
      unlistenAgent = await listen("agent_updated", () => {
        loadData();
      });
      unlistenTerminal = await listen("terminal_chunk", (event) => {
        const payload = event.payload as TerminalChunkEvent;
        terminalCursorBySession.set(payload.sessionId, payload.cursor);
        if (selectedSessionId === payload.sessionId && terminalWidget && payload.chunk) {
          terminalWidget.write(payload.chunk);
        }
      });
      unlistenSession = await listen("agent_runtime_updated", (event) => {
        const payload = event.payload as AgentRuntimeUpdatedEvent;
        if (payload.activeSessionId && selectedAgentId === payload.agentId) {
          selectedSessionId = payload.activeSessionId;
        }
        void loadData();
      });
      unlistenSessionPrompt = await listen("managed_session_prompt_required", async (event) => {
        const payload = event.payload as ManagedSessionPromptRequiredEvent;
        if (payload.reason === "missing_command") {
          await startSessionPrompt(selectedAgentId || undefined);
          return;
        }
        if (payload.reason === "confirmation_required" && payload.source === "voice") {
          const candidate = lastVoiceCommandText.trim() || voiceInput.trim();
          if (!candidate) return;
          const ok = window.confirm(payload.message ?? "Voice command requires confirmation.");
          if (!ok) return;
          await runVoiceText(candidate, { pushToTalk: true, confirmed: true });
        }
      });
      unlistenVoiceState = await listen("voice_state_updated", (event) => {
        const payload = event.payload as { state: string };
        voiceState = payload.state;
      });
      unlistenVoiceTranscript = await listen("voice_transcript", (event) => {
        const payload = event.payload as { text: string };
        lastTranscript = payload.text;
        lastVoiceCommandText = payload.text;
      });
      unlistenVoiceIntent = await listen("voice_intent", (event) => {
        const payload = event.payload as VoiceIntentEvent;
        lastIntent = `${payload.action} ${JSON.stringify(payload.payload)}`;
      });
      unlistenVoiceCommand = await listen("voice_action_executed", (event) => {
        const payload = event.payload as VoiceAction;
        lastCommand = `${payload.action} (${payload.result})`;
        if (payload.targetAgentId) {
          focusAgentById(payload.targetAgentId, payload.targetSessionId ?? undefined);
        }
        loadData();
      });
      unlistenVoiceError = await listen("voice_error", (event) => {
        const payload = event.payload as { message: string };
        lastCommand = `error: ${payload.message}`;
      });
      unlistenVoiceReply = await listen("voice_status_reply", (event) => {
        const payload = event.payload as VoiceStatusReplyEvent;
        lastCommand = payload.summary;
        if (payload.targetAgentId) {
          focusAgentById(payload.targetAgentId);
        }
      });
      unlistenAlertResolved = await listen("session_alert_resolved", () => {
        void loadData();
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
        void streamTerminalChunks(selectedSessionId);
      }
    }, 300);

    const resizeObserver =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(() => {
            fitAddon?.fit();
            if (selectedSessionId) {
              void resizeTerminalSession(selectedSessionId);
            }
          })
        : null;
    if (resizeObserver && terminalContainer) {
      resizeObserver.observe(terminalContainer);
    }

    window.addEventListener("keydown", handleGlobalAgentNavigation);

    return () => {
      clearInterval(interval);
      clearInterval(outputInterval);
      resizeObserver?.disconnect();
      terminalWidget?.dispose();
      terminalWidget = null;
      fitAddon = null;
      window.removeEventListener("keydown", handleGlobalAgentNavigation);
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
      unlistenAlertResolved?.();
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
      <button class="ghost" onclick={() => startSessionPrompt(selectedAgentId || undefined)}>
        Start session
      </button>
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
      placeholder="Type a test transcript (optional)"
      onkeydown={(event) => event.key === "Enter" && submitVoiceText()}
    />
    <button class="primary" onclick={submitVoiceText}>Run voice command</button>
    <button class="ghost" onclick={pushToTalkVoiceText} disabled={pushToTalkBusy}>
      {pushToTalkBusy ? "Push-to-talk running..." : "Push to talk"}
    </button>
    <p>Transcript: {lastTranscript || "none"}</p>
    <p>Intent: {lastIntent || "none"}</p>
    <p>Command: {lastCommand || "none"}</p>
  </section>

  <section class="layout">
    <AgentList
      {agents}
      selectedId={selectedAgentId}
      onSelect={focusAgentById}
      onAttach={attachFromAgent}
      onReply={replyFromAgent}
      onAcknowledge={acknowledgeFromAgent}
      {pendingAckAgentIds}
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

  <section class="timeline-panel">
    <header>
      <h2>Session timeline</h2>
      <span>
        {#if selectedSession}
          Session #{selectedSession.id}
        {:else}
          No session selected
        {/if}
      </span>
    </header>
    {#if !selectedSession}
      <p class="empty-timeline">Select an agent with a session to inspect timeline events.</p>
    {:else if sessionEvents.length === 0}
      <p class="empty-timeline">No events yet for this session.</p>
    {:else}
      <div class="timeline-list">
        {#each sessionEvents as event}
          <article class="timeline-item">
            <p class="timeline-head">
              <strong>{event.eventType}</strong>
              <span>{formatRelativeTime(event.createdAt)}</span>
            </p>
            {#if event.message}
              <p>{event.message}</p>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  </section>

  <section class="alerts-panel">
    <header>
      <h2>Input needed</h2>
      <span>{unresolvedAlerts.length} unresolved</span>
    </header>
    {#if unresolvedAlerts.length === 0}
      <p class="empty-alerts">No active input requests.</p>
    {:else}
      <div class="alerts-table">
        <div class="alerts-row header">
          <span>Agent</span>
          <span>Reason</span>
          <span>Message</span>
          <span>Created</span>
          <span>Actions</span>
        </div>
        {#each unresolvedAlerts as alert}
          <div class="alerts-row">
            <span>{lookupAgentName(alert.agentId)}</span>
            <span class="alert-reason">{alert.reason}</span>
            <span>{alert.message}</span>
            <span>{alert.createdAt}</span>
            <span class="actions">
              <button class="ghost small" onclick={() => focusSession(alert.sessionId)}>
                Open terminal
              </button>
              {#if alert.requiresAck && !alert.acknowledgedAt}
                <button class="ghost small" onclick={() => acknowledgeAlert(alert.id)}>Acknowledge</button>
              {/if}
              <button class="primary small" onclick={() => resolveAlert(alert.id)}>Resolve</button>
            </span>
          </div>
        {/each}
      </div>
    {/if}
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
                onclick={() => focusSession(session.id)}
              >
                Open terminal
              </button>
              <button class="ghost small" onclick={() => openSessionLogs(session.id)}>Open logs</button>
              {#if session.status === "active" || session.status === "stalled" || session.status === "waking" || session.status === "needs_input"}
                <button class="ghost small" onclick={() => stopSession(session.id)}>Stop</button>
              {/if}
              <button class="primary small" onclick={() => restartSession(session)}>Restart</button>
            </span>
          </div>
        {/each}
      {/if}
    </div>

  </section>

  <section class="terminal-panel">
    <header>
      <h2>Live terminal</h2>
      <span>{selectedSession ? `Session #${selectedSession.id}` : "No session selected"}</span>
    </header>
    <div class="terminal-widget" bind:this={terminalContainer}></div>
    {#if selectedAgent && selectedAgentAlerts.length > 0}
      <div class="selected-agent-alerts">
        <p>Selected agent unresolved alerts: {selectedAgentAlerts.length}</p>
      </div>
    {/if}
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

  .alerts-panel {
    margin-top: 20px;
    padding: 16px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .alerts-panel header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .empty-alerts {
    margin: 0;
    color: rgba(244, 242, 238, 0.7);
  }

  .alerts-table {
    display: grid;
    gap: 6px;
    max-height: 260px;
    overflow-y: auto;
  }

  .alerts-row {
    display: grid;
    grid-template-columns: 1fr 1fr 2fr 0.8fr 1.2fr;
    gap: 10px;
    align-items: center;
    padding: 10px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.03);
  }

  .alerts-row.header {
    font-size: 11px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: rgba(244, 242, 238, 0.6);
    background: transparent;
  }

  .alert-reason {
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-size: 12px;
    color: #ffd166;
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
  .status.needs_input,
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

  .timeline-panel {
    margin-top: 20px;
    padding: 16px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .timeline-panel header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .timeline-list {
    display: grid;
    gap: 8px;
    max-height: 220px;
    overflow-y: auto;
  }

  .timeline-item {
    padding: 10px 12px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
  }

  .timeline-head {
    margin: 0 0 6px;
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 10px;
  }

  .timeline-head span {
    color: rgba(244, 242, 238, 0.55);
    font-size: 12px;
  }

  .timeline-item p {
    margin: 0;
    font-size: 13px;
    color: rgba(244, 242, 238, 0.78);
  }

  .empty-timeline {
    margin: 0;
    color: rgba(244, 242, 238, 0.7);
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

  .terminal-widget {
    margin: 0 0 12px;
    height: 260px;
    overflow: auto;
    background: #07090d;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
    padding: 6px;
  }

  :global(.terminal-widget .xterm) {
    height: 100%;
  }

  :global(.terminal-widget .xterm-viewport) {
    border-radius: 8px;
  }

  .terminal-input-row {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 10px;
  }

  .selected-agent-alerts {
    margin-bottom: 12px;
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid rgba(255, 209, 102, 0.35);
    background: rgba(255, 209, 102, 0.08);
  }

  .selected-agent-alerts p {
    margin: 0;
    font-size: 12px;
    color: #ffd166;
    text-transform: uppercase;
    letter-spacing: 0.08em;
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

    .alerts-row {
      grid-template-columns: 1fr;
      gap: 6px;
    }
  }
</style>
