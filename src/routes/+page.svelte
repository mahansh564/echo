<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, tick } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";

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

  type BufferedTerminalChunk = {
    chunk: string;
    cursor: number;
  };

  type AgentListItem = {
    agent: AgentRow;
    session: SessionRuntime | null;
    status: SessionRuntime["status"] | "idle";
    isRunning: boolean;
    lastSeen: string;
  };

  let agents = $state<AgentRow[]>([]);
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
  let selectedSessionId = $state<number | null>(null);
  let attachedSessionId = $state<number | null>(null);
  let terminalInput = $state<string>("");
  let terminalContainer: HTMLDivElement | null = $state(null);
  let showClosedAgents = $state<boolean>(false);

  let terminalWidget: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  let terminalDataListener: { dispose: () => void } | null = null;
  let terminalFlushRaf = 0;

  const terminalViewportState: TerminalViewportState = {
    selectedSessionId: null,
    cursors: new Map<number, number>()
  };

  const terminalCursorBySession = terminalViewportState.cursors;
  const bufferedTerminalChunks = new Map<number, BufferedTerminalChunk[]>();
  const terminalResyncInFlight = new Set<number>();

  const TERMINAL_CHUNK_BYTES = 16_384;
  const TERMINAL_MAX_DRAIN_ITERATIONS = 8;
  const TERMINAL_FRAME_WRITE_BUDGET_BYTES = 32_768;
  const TERMINAL_MAX_PENDING_CHUNKS = 512;

  const ACTIVE_SESSION_STATUSES = new Set<SessionRuntime["status"]>([
    "waking",
    "active",
    "stalled",
    "needs_input"
  ]);
  const isActiveSessionStatus = (status: SessionRuntime["status"]) =>
    ACTIVE_SESSION_STATUSES.has(status);

  const selectedAgent = $derived(agents.find((agent) => agent.id === selectedAgentId));

  const selectedSession = $derived(
    selectedSessionId ? sessions.find((session) => session.id === selectedSessionId) : undefined
  );

  const activeSessions = $derived(
    sessions.filter((session) => isActiveSessionStatus(session.status))
  );

  const latestSessionByAgent = $derived(
    (() => {
      const map = new Map<number, SessionRuntime>();
      for (const session of sessions) {
        if (session.agentId === null || session.agentId === undefined) continue;
        if (!map.has(session.agentId)) {
          map.set(session.agentId, session);
        }
      }
      return map;
    })()
  );

  const agentListItems = $derived<AgentListItem[]>(
    agents
      .map((agent) => {
        const activeSession = agent.activeSessionId
          ? sessions.find((session) => session.id === agent.activeSessionId) ?? null
          : null;
        const session = activeSession ?? latestSessionByAgent.get(agent.id) ?? null;
        const status: SessionRuntime["status"] | "idle" = session?.status ?? "idle";
        const isRunning = status !== "ended" && status !== "failed" && status !== "idle";
        const lastSeen = formatRelativeTime(
          session?.lastActivityAt ?? session?.updatedAt ?? agent.lastActivityAt ?? agent.updatedAt
        );

        return {
          agent,
          session,
          status,
          isRunning,
          lastSeen
        };
      })
      .sort((a, b) => {
        if (a.isRunning !== b.isRunning) return a.isRunning ? -1 : 1;
        return a.agent.displayOrder - b.agent.displayOrder;
      })
  );

  const visibleAgentListItems = $derived<AgentListItem[]>(
    agentListItems.filter((item) => showClosedAgents || item.isRunning)
  );

  const toTitleCase = (value: string) =>
    value
      .split(/[_\s-]+/)
      .filter(Boolean)
      .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
      .join(" ");

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

  const applyRelativeTimes = (list: AgentRow[]) => {
    agents = list.map((agent) => ({
      ...agent,
      updatedAt: formatRelativeTime(agent.updatedAt),
      lastActivityAt: formatRelativeTime(agent.lastActivityAt ?? agent.updatedAt),
      lastInputRequiredAt: formatRelativeTime(agent.lastInputRequiredAt ?? null)
    }));
  };

  async function loadData() {
    loading = true;
    try {
      const [agentRowsResponse, sessionsResponse] = await Promise.all([
        invoke("list_agent_rows_cmd", { limit: 200 }),
        invoke("list_managed_sessions_cmd", { status: null, limit: 200 })
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

      applyRelativeTimes(mappedAgents);
      sessions = sessionsResponse as SessionRuntime[];

      if (agents.length > 0 && !agents.some((agent) => agent.id === selectedAgentId)) {
        selectedAgentId = agents[0].id;
      }
      if (agents.length === 0) {
        selectedAgentId = 0;
      }

      let nextSessionId = selectedSessionId;
      if (nextSessionId && !sessions.some((session) => session.id === nextSessionId)) {
        nextSessionId = null;
      }
      if (!nextSessionId) {
        nextSessionId =
          sessions.find((session) => isActiveSessionStatus(session.status))?.id ??
          sessions[0]?.id ??
          null;
      }

      if (nextSessionId !== selectedSessionId) {
        await setSelectedSession(nextSessionId, { reset: true });
      } else if (nextSessionId === null) {
        clearTerminalView();
      }
    } catch (error) {
      console.error("Failed to load data", error);
    } finally {
      loading = false;
    }
  }

  async function startSession(
    agentId?: number,
    taskId?: number,
    options: { command?: string | null } = {}
  ) {
    if (!agentId) return;
    const command = options.command?.trim() || "opencode";
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

  const lookupAgentName = (agentId?: number | null) =>
    agents.find((agent) => agent.id === agentId)?.name ?? "Unassigned";

  function upsertSessionRuntime(session: SessionRuntime) {
    const existing = sessions.find((entry) => entry.id === session.id);
    if (existing) {
      sessions = sessions.map((entry) => (entry.id === session.id ? session : entry));
      return;
    }
    sessions = [session, ...sessions];
  }

  async function attachTerminalSession(sessionId: number) {
    try {
      const updated = (await invoke("attach_terminal_session_cmd", { sessionId })) as SessionRuntime;
      upsertSessionRuntime(updated);
      attachedSessionId = sessionId;
    } catch (error) {
      console.error("Failed to attach terminal session", error);
    }
  }

  async function detachTerminalSession(sessionId: number) {
    try {
      const updated = (await invoke("detach_terminal_session_cmd", { sessionId })) as SessionRuntime;
      upsertSessionRuntime(updated);
      if (attachedSessionId === sessionId) {
        attachedSessionId = null;
      }
    } catch (error) {
      console.error("Failed to detach terminal session", error);
    }
  }

  async function setSelectedSession(sessionId: number | null, options: { reset?: boolean } = {}) {
    const { reset = true } = options;
    if (sessionId === null) {
      if (attachedSessionId) {
        await detachTerminalSession(attachedSessionId);
      }
      selectedSessionId = null;
      clearTerminalView();
      return;
    }

    if (attachedSessionId && attachedSessionId !== sessionId) {
      await detachTerminalSession(attachedSessionId);
    }

    selectedSessionId = sessionId;

    if (attachedSessionId !== sessionId) {
      await attachTerminalSession(sessionId);
    }

    fitAddon?.fit();
    await resizeTerminalSession(sessionId);

    if (reset || terminalCursorBySession.get(sessionId) === undefined) {
      await hydrateTerminalSession(sessionId, { reset: true });
    }
  }

  const findSessionForAgent = (agentId: number) =>
    sessions.find((session) => session.id === agents.find((agent) => agent.id === agentId)?.activeSessionId) ??
    sessions.find(
      (session) =>
        session.agentId === agentId && isActiveSessionStatus(session.status)
    );

  const focusAgentById = (agentId: number, preferredSessionId?: number | null) => {
    const target = agents.find((agent) => agent.id === agentId);
    if (!target) return;
    selectedAgentId = target.id;
    if (preferredSessionId) {
      void setSelectedSession(preferredSessionId, { reset: true });
      return;
    }
    const fallbackSession = findSessionForAgent(target.id);
    if (fallbackSession) {
      void setSelectedSession(fallbackSession.id, { reset: true });
    }
  };

  async function focusSession(sessionId: number) {
    const session = sessions.find((entry) => entry.id === sessionId);
    if (session?.agentId) {
      selectedAgentId = session.agentId;
    }
    await setSelectedSession(sessionId, { reset: true });
  }

  function clearTerminalView() {
    if (selectedSessionId) {
      bufferedTerminalChunks.delete(selectedSessionId);
    }
    terminalWidget?.clear();
    terminalWidget?.reset();
    if (terminalWidget) {
      terminalWidget.write("No terminal output yet.\r\n");
    }
  }

  function scheduleTerminalFlush() {
    if (terminalFlushRaf) return;
    terminalFlushRaf = window.requestAnimationFrame(() => {
      terminalFlushRaf = 0;
      flushTerminalChunks();
    });
  }

  function flushTerminalChunks() {
    if (!selectedSessionId || !terminalWidget) return;
    const queue = bufferedTerminalChunks.get(selectedSessionId);
    if (!queue || queue.length === 0) return;

    let writtenBytes = 0;
    while (queue.length > 0 && writtenBytes < TERMINAL_FRAME_WRITE_BUDGET_BYTES) {
      const next = queue.shift();
      if (!next) break;
      terminalWidget.write(next.chunk);
      terminalCursorBySession.set(selectedSessionId, next.cursor);
      writtenBytes += next.chunk.length;
    }

    if (queue.length === 0) {
      bufferedTerminalChunks.delete(selectedSessionId);
      return;
    }

    bufferedTerminalChunks.set(selectedSessionId, queue);
    scheduleTerminalFlush();
  }

  async function resyncTerminalStream(sessionId: number) {
    if (terminalResyncInFlight.has(sessionId)) return;
    terminalResyncInFlight.add(sessionId);
    try {
      bufferedTerminalChunks.delete(sessionId);
      await streamTerminalChunks(sessionId);
    } finally {
      terminalResyncInFlight.delete(sessionId);
    }
  }

  function queueTerminalChunk(payload: TerminalChunkEvent) {
    if (!selectedSessionId || payload.sessionId !== selectedSessionId || !payload.chunk) return;
    const queue = bufferedTerminalChunks.get(payload.sessionId) ?? [];
    queue.push({
      chunk: payload.chunk,
      cursor: payload.cursor
    });

    if (queue.length > TERMINAL_MAX_PENDING_CHUNKS) {
      bufferedTerminalChunks.delete(payload.sessionId);
      void resyncTerminalStream(payload.sessionId);
      return;
    }

    bufferedTerminalChunks.set(payload.sessionId, queue);
    scheduleTerminalFlush();
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
      bufferedTerminalChunks.delete(sessionId);
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
        convertEol: false,
        fontFamily: '"JetBrains Mono", "Menlo", monospace',
        fontSize: 12,
        lineHeight: 1.35,
        theme: {
          background: "#07090d",
          foreground: "#e7eef5",
          cursor: "#2fd4c3"
        }
      });

      terminalWidget.loadAddon(fitAddon);
      terminalWidget.open(terminalContainer);
      terminalDataListener = terminalWidget.onData((data) => {
        if (!selectedSessionId) return;
        invoke("send_terminal_data_cmd", {
          sessionId: selectedSessionId,
          data
        }).catch((error) => {
          console.error("Failed to send raw terminal data", error);
        });
      });

      await tick();
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

    void initTerminalWidget();

    const startListeners = async () => {
      unlistenTask = await listen("task_updated", () => {
        void loadData();
      });
      unlistenAgent = await listen("agent_updated", () => {
        void loadData();
      });
      unlistenTerminal = await listen("terminal_chunk", (event) => {
        const payload = event.payload as TerminalChunkEvent;
        queueTerminalChunk(payload);
      });
      unlistenSession = await listen("agent_runtime_updated", (event) => {
        const payload = event.payload as AgentRuntimeUpdatedEvent;
        if (payload.activeSessionId && selectedAgentId === payload.agentId) {
          void setSelectedSession(payload.activeSessionId, { reset: true });
        }
        void loadData();
      });
      unlistenSessionPrompt = await listen("managed_session_prompt_required", async (event) => {
        const payload = event.payload as ManagedSessionPromptRequiredEvent;
        if (payload.reason === "missing_command") {
          await startSession(selectedAgentId || undefined, selectedAgent?.taskId ?? undefined);
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
        void loadData();
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
    };

    void startListeners();

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
    void loadData();

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

    return () => {
      clearInterval(interval);
      resizeObserver?.disconnect();
      if (terminalFlushRaf) {
        window.cancelAnimationFrame(terminalFlushRaf);
        terminalFlushRaf = 0;
      }
      bufferedTerminalChunks.clear();
      terminalResyncInFlight.clear();
      terminalDataListener?.dispose();
      terminalDataListener = null;
      if (attachedSessionId) {
        void invoke("detach_terminal_session_cmd", { sessionId: attachedSessionId });
      }
      terminalWidget?.dispose();
      terminalWidget = null;
      fitAddon = null;
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

<main class="app-shell">
  <section class="workspace">
    <section class="terminals-pane">
      <header class="pane-header">
        <h1>Current terminals</h1>
        <div class="pane-header-actions">
          <span>{activeSessions.length}</span>
          <button
            class="primary"
            onclick={() => startSession(selectedAgent?.id, selectedAgent?.taskId ?? undefined)}
            disabled={!selectedAgent}
          >
            New session
          </button>
        </div>
      </header>

      <div class="terminal-session-list">
        {#if activeSessions.length === 0}
          <p class="empty-state">No active sessions right now.</p>
        {:else}
          {#each activeSessions as session}
            <button
              class="session-card"
              class:selected={session.id === selectedSessionId}
              onclick={() => focusSession(session.id)}
            >
              <div class="session-card-head">
                <strong>Session #{session.id}</strong>
                <span class={`status ${session.status}`}>{toTitleCase(session.status)}</span>
              </div>
              <p>{lookupAgentName(session.agentId)} · {session.launchCommand}</p>
              <p>Last activity: {formatRelativeTime(session.lastActivityAt ?? session.updatedAt)}</p>
            </button>
          {/each}
        {/if}
      </div>

      <div class="terminal-view">
        <header class="terminal-view-header">
          <strong>
            {#if selectedSession}
              Session #{selectedSession.id} · {lookupAgentName(selectedSession.agentId)}
            {:else}
              No session selected
            {/if}
          </strong>
          {#if selectedSession}
            <span class={`status ${selectedSession.status}`}>{toTitleCase(selectedSession.status)}</span>
          {/if}
        </header>

        <div class="terminal-widget" bind:this={terminalContainer}></div>

        <div class="terminal-input-row">
          <input
            bind:value={terminalInput}
            placeholder="Send input to selected terminal"
            onkeydown={(event) => event.key === "Enter" && sendTerminalInput()}
            disabled={!selectedSessionId}
          />
          <button class="primary" onclick={sendTerminalInput} disabled={!selectedSessionId}>Send</button>
        </div>
      </div>
    </section>

    <aside class="activity-pane">
      <header class="pane-header">
        <h2>Agents</h2>
        <div class="pane-header-actions">
          <span>{visibleAgentListItems.length}</span>
          <button class="ghost" onclick={() => (showClosedAgents = !showClosedAgents)}>
            {showClosedAgents ? "Hide closed" : "Show closed"}
          </button>
        </div>
      </header>

      <div class="activity-list">
        {#if visibleAgentListItems.length === 0}
          <p class="empty-state">No running agents right now.</p>
        {:else}
          {#each visibleAgentListItems as item (item.agent.id)}
            <article class="activity-item">
              <div class="agent-item-head">
                <p class="activity-title">{item.agent.name}</p>
                <span class={`status status-pill ${item.status}`}>{toTitleCase(item.status)}</span>
              </div>
              <p class="activity-meta">Last seen: {item.lastSeen}</p>
              {#if item.session}
                <p class="activity-message">
                  Session #{item.session.id} · {item.session.launchCommand}
                </p>
                <div class="activity-actions">
                  <button class="ghost" onclick={() => focusAgentById(item.agent.id, item.session?.id)}>
                    Open
                  </button>
                </div>
              {:else}
                <p class="activity-message">No sessions yet.</p>
              {/if}
            </article>
          {/each}
        {/if}
      </div>
    </aside>
  </section>

  <section class="voice-toolbar">
    <div class="voice-metrics">
      <strong>Voice: {voiceState}</strong>
      <span>Transcript: {lastTranscript || "none"}</span>
      <span>Intent: {lastIntent || "none"}</span>
      <span>Command: {lastCommand || "none"}</span>
    </div>

    <div class="voice-actions">
      {#if voiceRunning}
        <button class="ghost" onclick={stopVoice}>Stop voice</button>
      {:else}
        <button class="ghost" onclick={startVoice}>Start voice</button>
      {/if}
      <button class="ghost" onclick={pushToTalkVoiceText} disabled={pushToTalkBusy}>
        {pushToTalkBusy ? "Listening..." : "Push to talk"}
      </button>
    </div>

    <div class="voice-input">
      <input
        bind:value={voiceInput}
        placeholder="Type voice command"
        onkeydown={(event) => event.key === "Enter" && submitVoiceText()}
      />
      <button class="primary" onclick={submitVoiceText}>Run</button>
    </div>
  </section>

  {#if loading}
    <p class="loading">Refreshing…</p>
  {/if}
</main>

<style>
  :global(html),
  :global(body) {
    height: 100%;
    margin: 0;
    overflow: hidden;
    background: #0b1118;
    color: #dde7ef;
    font-family: "Space Grotesk", "Avenir Next", "Segoe UI", sans-serif;
  }

  * {
    box-sizing: border-box;
  }

  .app-shell {
    height: 100vh;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    overflow: hidden;
    background-image: radial-gradient(circle at 12% 10%, rgba(47, 212, 195, 0.12), transparent 34%),
      radial-gradient(circle at 92% 2%, rgba(255, 184, 92, 0.12), transparent 36%);
  }

  .workspace {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: grid;
    grid-template-columns: minmax(560px, 1.9fr) minmax(280px, 1fr);
    gap: 12px;
  }

  .terminals-pane,
  .activity-pane {
    min-height: 0;
    background: rgba(8, 14, 22, 0.88);
    border: 1px solid rgba(132, 162, 194, 0.25);
    border-radius: 14px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    backdrop-filter: blur(12px);
  }

  .activity-pane {
    background: linear-gradient(180deg, rgba(10, 18, 28, 0.94) 0%, rgba(8, 14, 22, 0.94) 100%);
    border-color: rgba(127, 164, 202, 0.36);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03);
  }

  .pane-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .pane-header h1,
  .pane-header h2 {
    margin: 0;
    font-size: 16px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .pane-header span {
    font-size: 12px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: rgba(221, 231, 239, 0.68);
  }

  .pane-header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .terminal-session-list {
    display: grid;
    gap: 8px;
    max-height: 220px;
    min-height: 110px;
    overflow-y: auto;
    padding-right: 4px;
  }

  .session-card {
    width: 100%;
    border: 1px solid transparent;
    background: rgba(16, 24, 34, 0.85);
    border-radius: 12px;
    padding: 10px;
    color: inherit;
    text-align: left;
    cursor: pointer;
    display: grid;
    gap: 4px;
  }

  .session-card:hover {
    border-color: rgba(47, 212, 195, 0.45);
  }

  .session-card.selected {
    border-color: rgba(47, 212, 195, 0.9);
    background: rgba(19, 36, 48, 0.95);
  }

  .session-card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .session-card p {
    margin: 0;
    font-size: 12px;
    color: rgba(221, 231, 239, 0.78);
  }

  .terminal-view {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .terminal-view-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    color: rgba(221, 231, 239, 0.88);
  }

  .terminal-widget {
    flex: 1;
    min-height: 300px;
    overflow: hidden;
    background: #060b12;
    border: 1px solid rgba(123, 161, 199, 0.3);
    border-radius: 12px;
    padding: 0;
  }

  :global(.terminal-widget .xterm) {
    width: 100%;
    height: 100%;
  }

  .terminal-input-row {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 8px;
  }

  .terminal-input-row input,
  .voice-input input {
    border: 1px solid rgba(123, 161, 199, 0.45);
    border-radius: 10px;
    padding: 10px 12px;
    background: rgba(11, 19, 29, 0.95);
    color: inherit;
  }

  .activity-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    grid-template-columns: 1fr;
    grid-auto-rows: 120px;
    align-content: start;
    gap: 8px;
    padding-right: 4px;
  }

  .activity-item {
    height: 120px;
    border-radius: 12px;
    padding: 10px 11px;
    background: linear-gradient(165deg, rgba(17, 28, 41, 0.96) 0%, rgba(12, 20, 31, 0.96) 100%);
    border: 1px solid rgba(122, 155, 187, 0.34);
    display: flex;
    flex-direction: column;
    gap: 6px;
    overflow: hidden;
    transition: border-color 140ms ease, transform 140ms ease, box-shadow 140ms ease;
  }

  .activity-item:hover {
    border-color: rgba(47, 212, 195, 0.56);
    transform: translateY(-1px);
    box-shadow: 0 6px 14px rgba(3, 10, 18, 0.35);
  }

  .agent-item-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 8px;
    min-height: 22px;
  }

  .activity-title,
  .activity-meta,
  .activity-message {
    margin: 0;
  }

  .activity-title {
    font-size: 13px;
    font-weight: 700;
    letter-spacing: 0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .activity-meta {
    font-size: 11px;
    color: rgba(221, 231, 239, 0.7);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .activity-message {
    font-size: 12px;
    color: rgba(226, 235, 242, 0.9);
    display: -webkit-box;
    line-clamp: 1;
    -webkit-line-clamp: 1;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .activity-actions {
    display: flex;
    gap: 8px;
    margin-top: auto;
  }

  .status-pill {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    border: 1px solid rgba(128, 160, 190, 0.5);
    background: rgba(15, 24, 35, 0.95);
    padding: 2px 8px;
    font-size: 10px;
    letter-spacing: 0.06em;
    white-space: nowrap;
  }

  .voice-toolbar {
    display: grid;
    grid-template-columns: 1.3fr auto 1fr;
    gap: 10px;
    align-items: center;
    background: rgba(6, 10, 16, 0.95);
    border: 1px solid rgba(123, 161, 199, 0.32);
    border-radius: 14px;
    padding: 10px;
    position: sticky;
    bottom: 0;
  }

  .voice-metrics {
    display: grid;
    gap: 3px;
    min-width: 0;
  }

  .voice-metrics strong,
  .voice-metrics span {
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .voice-actions,
  .voice-input {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .voice-input {
    min-width: 0;
  }

  .voice-input input {
    flex: 1;
    min-width: 0;
  }

  button {
    border: 1px solid rgba(118, 152, 184, 0.36);
    border-radius: 10px;
    padding: 8px 12px;
    font-weight: 700;
    cursor: pointer;
    font-size: 12px;
    transition: border-color 130ms ease, transform 130ms ease, opacity 130ms ease;
  }

  button:not(:disabled):hover {
    transform: translateY(-1px);
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .ghost {
    background: rgba(133, 161, 190, 0.2);
    color: #e5edf4;
  }

  .primary {
    background: linear-gradient(180deg, #3ae0cd 0%, #2fd4c3 100%);
    color: #042926;
    border-color: rgba(47, 212, 195, 0.52);
  }

  .status {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .status.active {
    color: #2fd4c3;
  }

  .status.waking,
  .status.stalled {
    color: #ffb85c;
  }

  .status.needs_input,
  .status.failed {
    color: #ff7b72;
  }

  .status.ended {
    color: rgba(221, 231, 239, 0.56);
  }

  .status.idle {
    color: rgba(221, 231, 239, 0.56);
  }

  .status-pill.active {
    border-color: rgba(47, 212, 195, 0.6);
    background: rgba(11, 44, 40, 0.8);
  }

  .status-pill.waking,
  .status-pill.stalled {
    border-color: rgba(255, 184, 92, 0.62);
    background: rgba(58, 43, 20, 0.78);
  }

  .status-pill.needs_input,
  .status-pill.failed {
    border-color: rgba(255, 123, 114, 0.62);
    background: rgba(58, 24, 22, 0.78);
  }

  .status-pill.ended,
  .status-pill.idle {
    border-color: rgba(160, 173, 186, 0.5);
    background: rgba(31, 41, 51, 0.75);
  }

  .empty-state {
    margin: 0;
    font-size: 13px;
    color: rgba(221, 231, 239, 0.72);
    padding: 8px 2px;
  }

  .loading {
    margin: 0;
    font-size: 12px;
    color: rgba(221, 231, 239, 0.7);
  }

  @media (max-width: 1080px) {
    .workspace {
      grid-template-columns: 1fr;
    }

    .voice-toolbar {
      grid-template-columns: 1fr;
      align-items: stretch;
    }

    .voice-actions,
    .voice-input {
      width: 100%;
    }

    .voice-actions button,
    .voice-input button {
      flex: 1;
    }

    .activity-list {
      grid-auto-rows: 112px;
    }

    .activity-item {
      height: 112px;
    }
  }
</style>
