<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
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

  type RuntimeIssueKind = "adapter_down" | "model_down" | "mic_unavailable";

  type RuntimeIssueSeverity = "warning" | "critical";

  type RuntimeIssueSource = "voice" | "terminal" | "session_alert" | "system";

  type RuntimeIssueRecord = {
    kind: RuntimeIssueKind;
    source: RuntimeIssueSource;
    rawMessage: string;
    enrichedMessage?: string | null;
    enrichmentStatus: "pending" | "success" | "failed" | string;
    enrichmentError?: string | null;
    firstSeenAt: string;
    lastSeenAt: string;
    seenCount: number;
    dismissedUntil?: string | null;
    resolvedAt?: string | null;
  };

  type RuntimeIssue = {
    kind: RuntimeIssueKind;
    severity: RuntimeIssueSeverity;
    title: string;
    guidance: string;
    message: string;
    source: RuntimeIssueSource;
    firstSeenAt: string;
    lastSeenAt: string;
    count: number;
  };

  type SessionAlert = {
    id: number;
    sessionId: number;
    agentId?: number | null;
    severity: string;
    reason: string;
    message: string;
    messageEnriched?: string | null;
    messageEnrichmentStatus: "pending" | "success" | "failed" | string;
    messageEnrichedAt?: string | null;
    messageEnrichmentError?: string | null;
    requiresAck: boolean;
    acknowledgedAt?: string | null;
    snoozedUntil?: string | null;
    escalatedAt?: string | null;
    escalationCount?: number;
    resolvedAt?: string | null;
    createdAt: string;
    updatedAt: string;
  };

  type SessionAlertCreatedEvent = {
    alertId: number;
    sessionId: number;
    agentId?: number | null;
    severity: string;
    reason: string;
    message: string;
    messageEnriched?: string | null;
    messageEnrichmentStatus: "pending" | "success" | "failed" | string;
    messageEnrichedAt?: string | null;
    requiresAck: boolean;
    createdAt: string;
  };

  type SessionAlertResolvedEvent = {
    alertId: number;
    sessionId: number;
    agentId?: number | null;
    resolvedAt?: string | null;
  };

  type SessionAlertSnoozedEvent = {
    alertId: number;
    sessionId: number;
    agentId?: number | null;
    snoozedUntil?: string | null;
  };

  type SessionAlertEscalatedEvent = {
    alertId: number;
    sessionId: number;
    agentId?: number | null;
    severity: string;
    escalatedAt?: string | null;
    escalationCount?: number;
  };

  type AlertToast = {
    id: string;
    alertId: number;
    sessionId: number;
    agentId?: number | null;
    severity: string;
    reason: string;
    message: string;
  };

  type TerminalViewportState = {
    selectedSessionId: number | null;
    cursors: Map<number, number>;
  };

  type BufferedTerminalChunk = {
    chunk: string;
    cursor: number;
  };

  type UnlistenFn = () => void;

  type PaletteCommandId =
    | "show-unresolved-inputs"
    | "voice-query-input-needed"
    | "switch-mode-zen"
    | "switch-mode-full";

  type PaletteCommand = {
    id: PaletteCommandId;
    label: string;
    meta: string;
  };

  type PaletteEntry =
    | {
        id: string;
        kind: "command";
        label: string;
        meta: string;
        commandId: PaletteCommandId;
      }
    | {
        id: string;
        kind: "session";
        label: string;
        meta: string;
        sessionId: number;
      }
    | {
        id: string;
        kind: "alert";
        label: string;
        meta: string;
        alert: SessionAlert;
      };

  const COMMAND_PALETTE_COMMANDS: PaletteCommand[] = [
    {
      id: "show-unresolved-inputs",
      label: "Show unresolved input alerts",
      meta: "Refresh unresolved alerts from active sessions"
    },
    {
      id: "voice-query-input-needed",
      label: "Ask voice: which agents need input",
      meta: "Runs the voice query for unresolved input requests"
    },
    {
      id: "switch-mode-zen",
      label: "Switch to Zen mode",
      meta: "Hide dashboard and move to menubar-focused workflow"
    },
    {
      id: "switch-mode-full",
      label: "Switch to Full mode",
      meta: "Show dashboard mode as the primary app window"
    }
  ];

  let agents = $state<AgentRow[]>([]);
  let sessions = $state<SessionRuntime[]>([]);
  let selectedAgentId = $state<number>(0);
  let loading = $state<boolean>(true);
  let hasLoadedOnce = $state<boolean>(false);
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
  let showCommandPalette = $state<boolean>(false);
  let paletteQuery = $state<string>("");
  let paletteSelectedIndex = $state<number>(0);
  let paletteInput: HTMLInputElement | null = $state(null);
  let unresolvedAlerts = $state<SessionAlert[]>([]);
  let unresolvedAlertsLoading = $state<boolean>(false);
  let alertActionBusyId = $state<number | null>(null);
  let alertToasts = $state<AlertToast[]>([]);
  let runtimeIssueRecords = $state<RuntimeIssueRecord[]>([]);

  let terminalWidget: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  let terminalDataListener: { dispose: () => void } | null = null;
  let terminalFlushRaf = 0;
  let terminalCursorPersistTimer = 0;
  let inputRequestRefreshTimer = 0;
  let listenerReconnectTimer = 0;
  let listenerReconnectAttempts = 0;
  let listenerLifecycleStopped = false;
  let uiUnlisteners: UnlistenFn[] = [];

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
  const TERMINAL_POPOUT_WINDOW_LABEL = "terminal-popout";
  const TERMINAL_CURSOR_STORAGE_KEY = "echo.main.terminal-cursors.v1";
  const SELECTED_SESSION_STORAGE_KEY = "echo.main.selected-session.v1";
  const TERMINAL_CURSOR_PERSIST_DEBOUNCE_MS = 250;
  const INPUT_REQUEST_REFRESH_DEBOUNCE_MS = 300;
  const LISTENER_RECONNECT_BASE_MS = 1000;
  const LISTENER_RECONNECT_MAX_MS = 15000;
  const ALERT_TOAST_MAX = 4;
  const ALERT_TOAST_TTL_MS = 8000;
  const UI_SNIPPET_MAX_CHARS = 180;
  const UI_RUNTIME_ISSUE_MAX_CHARS = 240;
  const UI_VOICE_LINE_MAX_CHARS = 220;
  const RUNTIME_ISSUE_MIC_PATTERNS = [
    "microphone",
    "audio device",
    "avfoundation",
    "ffmpeg capture failed",
    "permission",
    "input device",
    "device not found"
  ];
  const RUNTIME_ISSUE_MODEL_PATTERNS = [
    "asr model",
    "asr sidecar",
    "asr endpoint",
    "model endpoint",
    "/api/generate",
    "transcribe",
    "error sending request",
    "connection refused",
    "timed out",
    "dns",
    "reqwest",
    "llama"
  ];
  const RUNTIME_ISSUE_ADAPTER_PATTERNS = [
    "failed to spawn command",
    "command is required",
    "failed to open pty",
    "provider parse error",
    "adapter parse error",
    "spawn command",
    "no such file or directory",
    "session start failed"
  ];

  const RUNTIME_ISSUE_META: Record<
    RuntimeIssueKind,
    {
      title: string;
      guidance: string;
      severity: RuntimeIssueSeverity;
    }
  > = {
    adapter_down: {
      title: "Adapter unavailable",
      guidance: "Verify the provider CLI is installed and runnable from this machine.",
      severity: "critical"
    },
    model_down: {
      title: "Model unavailable",
      guidance: "Check ASR/LLM endpoint reachability and local model/sidecar paths.",
      severity: "warning"
    },
    mic_unavailable: {
      title: "Microphone unavailable",
      guidance: "Confirm microphone permissions and selected audio input device.",
      severity: "critical"
    }
  };

  const alertToastTimers = new Map<string, number>();

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

  const openAgentSessions = $derived.by(() =>
    [...activeSessions].sort((left, right) => {
      const leftAt = left.lastActivityAt ?? left.updatedAt;
      const rightAt = right.lastActivityAt ?? right.updatedAt;
      return new Date(rightAt).getTime() - new Date(leftAt).getTime();
    })
  );

  const needsInputSessionIds = $derived.by(() => {
    const ids = new Set<number>();
    for (const session of sessions) {
      if (session.needsInput || session.status === "needs_input") {
        ids.add(session.id);
      }
    }
    return ids;
  });

  const visibleInputRequests = $derived(
    unresolvedAlerts.filter(
      (alert) => !alert.acknowledgedAt && needsInputSessionIds.has(alert.sessionId)
    )
  );

  const activeRuntimeIssues = $derived.by(() =>
    runtimeIssueRecords
      .map((issue): RuntimeIssue => {
        const meta = RUNTIME_ISSUE_META[issue.kind];
        return {
          kind: issue.kind,
          severity: meta.severity,
          title: meta.title,
          guidance: meta.guidance,
          message: sanitizeDisplayText(
            issue.enrichedMessage ?? issue.rawMessage,
            UI_RUNTIME_ISSUE_MAX_CHARS
          ),
          source: issue.source,
          firstSeenAt: issue.firstSeenAt,
          lastSeenAt: issue.lastSeenAt,
          count: issue.seenCount
        };
      })
      .sort((left, right) => {
        if (left.severity !== right.severity) {
          return left.severity === "critical" ? -1 : 1;
        }
        return (
          new Date(right.lastSeenAt).getTime() - new Date(left.lastSeenAt).getTime()
        );
      })
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
      lastInputRequiredAt: formatRelativeTime(agent.lastInputRequiredAt ?? null),
      lastSnippet: sanitizeOptionalText(agent.lastSnippet ?? null, UI_SNIPPET_MAX_CHARS)
    }));
  };

  const parsePositiveSessionId = (value: unknown) => {
    if (typeof value !== "number" || !Number.isInteger(value) || value <= 0) {
      return null;
    }
    return value;
  };

  const includesAnyPattern = (value: string, patterns: string[]) =>
    patterns.some((pattern) => value.includes(pattern));

  const sanitizeDisplayText = (value: unknown, maxChars = UI_VOICE_LINE_MAX_CHARS) => {
    if (value === null || value === undefined) return "";
    const text = String(value)
      .replace(/\u001B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])/g, "")
      .replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, " ")
      .replace(/\s+/g, " ")
      .trim();
    if (!text) return "";
    if (text.length <= maxChars) return text;
    if (maxChars <= 1) return "…";
    return `${text.slice(0, maxChars - 1)}…`;
  };

  const sanitizeOptionalText = (value: string | null | undefined, maxChars: number) => {
    if (!value) return null;
    const cleaned = sanitizeDisplayText(value, maxChars);
    return cleaned || null;
  };

  const displayAlertMessage = (alert: {
    message: string;
    messageEnriched?: string | null;
  }) => sanitizeDisplayText(alert.messageEnriched ?? alert.message, UI_SNIPPET_MAX_CHARS);

  const summarizeVoiceIntent = (event: VoiceIntentEvent) => {
    const agentIndex = event.payload?.agent_index;
    const agentHint = event.payload?.agent_name_hint;
    const input = event.payload?.input;
    const command = event.payload?.command;
    const parts: string[] = [event.action];

    if (typeof agentIndex === "number" && Number.isFinite(agentIndex)) {
      parts.push(`agent ${agentIndex}`);
    } else if (typeof agentHint === "string" && agentHint.trim()) {
      parts.push(`agent ${sanitizeDisplayText(agentHint, 40)}`);
    }

    if (typeof command === "string" && command.trim()) {
      parts.push(`cmd ${sanitizeDisplayText(command, 80)}`);
    }
    if (typeof input === "string" && input.trim()) {
      parts.push(`input ${sanitizeDisplayText(input, 80)}`);
    }

    return sanitizeDisplayText(parts.join(" · "), UI_VOICE_LINE_MAX_CHARS);
  };

  const normalizeErrorMessage = (error: unknown) => {
    if (typeof error === "string") {
      return sanitizeDisplayText(error, UI_RUNTIME_ISSUE_MAX_CHARS);
    }
    if (error && typeof error === "object" && "message" in error) {
      const maybeMessage = (error as { message?: unknown }).message;
      if (typeof maybeMessage === "string") {
        return sanitizeDisplayText(maybeMessage, UI_RUNTIME_ISSUE_MAX_CHARS);
      }
    }
    try {
      return sanitizeDisplayText(JSON.stringify(error), UI_RUNTIME_ISSUE_MAX_CHARS);
    } catch {
      return sanitizeDisplayText(String(error), UI_RUNTIME_ISSUE_MAX_CHARS);
    }
  };

  const classifyRuntimeIssue = (
    message: string,
    forcedKind?: RuntimeIssueKind
  ): RuntimeIssueKind | null => {
    if (forcedKind) return forcedKind;
    const normalized = message.toLowerCase();
    if (includesAnyPattern(normalized, RUNTIME_ISSUE_MIC_PATTERNS)) {
      return "mic_unavailable";
    }
    if (includesAnyPattern(normalized, RUNTIME_ISSUE_MODEL_PATTERNS)) {
      return "model_down";
    }
    if (includesAnyPattern(normalized, RUNTIME_ISSUE_ADAPTER_PATTERNS)) {
      return "adapter_down";
    }
    return null;
  };

  async function clearRuntimeIssue(kind: RuntimeIssueKind) {
    try {
      await invoke("clear_runtime_issue_cmd", { kind });
      runtimeIssueRecords = runtimeIssueRecords.filter((issue) => issue.kind !== kind);
    } catch (error) {
      console.error("Failed to clear runtime issue", error);
    }
  }

  async function dismissRuntimeIssue(kind: RuntimeIssueKind) {
    try {
      await invoke("dismiss_runtime_issue_cmd", { kind });
      runtimeIssueRecords = runtimeIssueRecords.filter((issue) => issue.kind !== kind);
    } catch (error) {
      console.error("Failed to dismiss runtime issue", error);
    }
  }

  async function reportRuntimeIssue(input: {
    error: unknown;
    source: RuntimeIssueSource;
    forcedKind?: RuntimeIssueKind;
  }) {
    const message = normalizeErrorMessage(input.error).trim();
    if (!message) return;
    const kind = classifyRuntimeIssue(message, input.forcedKind);
    if (!kind) return;
    try {
      const updated = (await invoke("report_runtime_issue_cmd", {
        kind,
        source: input.source,
        message
      })) as RuntimeIssueRecord;
      runtimeIssueRecords = [
        updated,
        ...runtimeIssueRecords.filter((issue) => issue.kind !== updated.kind)
      ].slice(0, 200);
    } catch (error) {
      console.error("Failed to report runtime issue", error);
    }
  }

  function restoreTerminalViewportState() {
    if (typeof window === "undefined") return;

    const storedSelectedSession = window.sessionStorage.getItem(SELECTED_SESSION_STORAGE_KEY);
    if (storedSelectedSession) {
      const parsed = Number.parseInt(storedSelectedSession, 10);
      const sessionId = parsePositiveSessionId(parsed);
      if (sessionId) {
        selectedSessionId = sessionId;
      }
    }

    const rawCursorState = window.sessionStorage.getItem(TERMINAL_CURSOR_STORAGE_KEY);
    if (!rawCursorState) return;
    try {
      const parsed = JSON.parse(rawCursorState) as Record<string, unknown>;
      for (const [key, value] of Object.entries(parsed)) {
        const sessionId = parsePositiveSessionId(Number.parseInt(key, 10));
        if (!sessionId) continue;
        const cursor =
          typeof value === "number" && Number.isFinite(value) && value >= 0
            ? Math.floor(value)
            : null;
        if (cursor === null) continue;
        terminalCursorBySession.set(sessionId, cursor);
      }
    } catch (error) {
      console.error("Failed to restore terminal cursor state", error);
    }
  }

  function persistSelectedSession(sessionId: number | null) {
    if (typeof window === "undefined") return;
    if (!sessionId) {
      window.sessionStorage.removeItem(SELECTED_SESSION_STORAGE_KEY);
      return;
    }
    window.sessionStorage.setItem(SELECTED_SESSION_STORAGE_KEY, String(sessionId));
  }

  function persistTerminalCursors() {
    if (typeof window === "undefined") return;
    const cursorState: Record<string, number> = {};
    for (const [sessionId, cursor] of terminalCursorBySession.entries()) {
      if (!Number.isFinite(cursor) || cursor < 0) continue;
      cursorState[String(sessionId)] = Math.floor(cursor);
    }
    if (Object.keys(cursorState).length === 0) {
      window.sessionStorage.removeItem(TERMINAL_CURSOR_STORAGE_KEY);
      return;
    }
    window.sessionStorage.setItem(TERMINAL_CURSOR_STORAGE_KEY, JSON.stringify(cursorState));
  }

  function schedulePersistTerminalCursors() {
    if (typeof window === "undefined") return;
    if (terminalCursorPersistTimer) return;
    terminalCursorPersistTimer = window.setTimeout(() => {
      terminalCursorPersistTimer = 0;
      persistTerminalCursors();
    }, TERMINAL_CURSOR_PERSIST_DEBOUNCE_MS);
  }

  function scheduleInputRequestRefresh() {
    if (typeof window === "undefined") return;
    if (inputRequestRefreshTimer) return;
    inputRequestRefreshTimer = window.setTimeout(() => {
      inputRequestRefreshTimer = 0;
      void loadData({ background: true });
    }, INPUT_REQUEST_REFRESH_DEBOUNCE_MS);
  }

  const listenerReconnectDelayMs = (attempt: number) =>
    Math.min(
      LISTENER_RECONNECT_MAX_MS,
      LISTENER_RECONNECT_BASE_MS * Math.pow(2, Math.max(0, attempt - 1))
    );

  function clearUiListeners() {
    for (const unlisten of uiUnlisteners) {
      unlisten();
    }
    uiUnlisteners = [];
  }

  async function loadData(options: { background?: boolean } = {}) {
    const background = options.background ?? hasLoadedOnce;
    if (!background) {
      loading = true;
    }
    try {
      const [agentRowsResponse, sessionsResponse, alertsResponse, runtimeIssuesResponse] =
        await Promise.all([
        invoke("list_agent_rows_cmd", { limit: 200 }),
        invoke("list_managed_sessions_cmd", { status: null, limit: 200 }),
        invoke("list_session_alerts_cmd", {
          agentId: null,
          unresolvedOnly: true,
          limit: 200
        }),
        invoke("list_runtime_issues_cmd", { limit: 200 })
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
        lastSnippet: sanitizeOptionalText(row.lastSnippet ?? null, UI_SNIPPET_MAX_CHARS),
        updatedAt: row.updatedAt
      }));

      applyRelativeTimes(mappedAgents);
      sessions = sessionsResponse as SessionRuntime[];
      unresolvedAlerts = (alertsResponse as SessionAlert[]).map((alert) => ({
        ...alert,
        message: sanitizeDisplayText(alert.message, UI_SNIPPET_MAX_CHARS),
        messageEnriched: sanitizeOptionalText(alert.messageEnriched ?? null, UI_SNIPPET_MAX_CHARS)
      }));
      runtimeIssueRecords = runtimeIssuesResponse as RuntimeIssueRecord[];

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
        await setSelectedSession(nextSessionId, {
          reset: nextSessionId !== null ? !terminalCursorBySession.has(nextSessionId) : true
        });
      } else if (nextSessionId !== null && attachedSessionId !== nextSessionId) {
        await setSelectedSession(nextSessionId, {
          reset: !terminalCursorBySession.has(nextSessionId)
        });
      } else if (nextSessionId === null) {
        clearTerminalView();
      }
      hasLoadedOnce = true;
    } catch (error) {
      console.error("Failed to load data", error);
      unresolvedAlerts = [];
      await reportRuntimeIssue({ error, source: "system" });
    } finally {
      if (!background) {
        loading = false;
      }
    }
  }

  async function startSession(
    agentId?: number,
    taskId?: number,
    options: { command?: string | null; provider?: string | null } = {}
  ) {
    if (!agentId) return;
    const targetAgent = agents.find((agent) => agent.id === agentId);
    const provider = options.provider?.trim() || targetAgent?.provider || "opencode";
    const command = options.command?.trim() ?? "";
    try {
      await invoke("start_agent_session_cmd", {
        agentId,
        launchProfile: {
          command: command.length > 0 ? command : null,
          args: [],
          cwd: null,
          taskId: taskId ?? null,
          provider
        }
      });
      await clearRuntimeIssue("adapter_down");
      await loadData();
    } catch (error) {
      console.error("Failed to start managed session", error);
      await reportRuntimeIssue({
        error,
        source: "terminal",
        forcedKind: "adapter_down"
      });
    }
  }

  const lookupAgentName = (agentId?: number | null) =>
    agents.find((agent) => agent.id === agentId)?.name ?? "Unassigned";

  const lookupAgentTaskTitle = (agentId?: number | null) =>
    agents.find((agent) => agent.id === agentId)?.taskTitle ?? "Unassigned";

  const filterIncludes = (value: string, query: string) => value.toLowerCase().includes(query);

  const paletteEntries = $derived.by((): PaletteEntry[] => {
    const query = paletteQuery.trim().toLowerCase();
    const commands = COMMAND_PALETTE_COMMANDS.filter(
      (command) =>
        !query || filterIncludes(command.label, query) || filterIncludes(command.meta, query)
    ).map((command) => ({
      id: `command-${command.id}`,
      kind: "command" as const,
      label: command.label,
      meta: command.meta,
      commandId: command.id
    }));

    const sessionsForPalette = [...activeSessions];
    if (
      selectedSession &&
      !sessionsForPalette.some((session) => session.id === selectedSession.id)
    ) {
      sessionsForPalette.unshift(selectedSession);
    }

    const sessionEntries = sessionsForPalette
      .filter((session) => {
        if (!query) return true;
        const agentName = lookupAgentName(session.agentId);
        const haystack =
          `session ${session.id} ${session.status} ${session.launchCommand} ${agentName}`.toLowerCase();
        return haystack.includes(query);
      })
      .map((session) => ({
        id: `session-${session.id}`,
        kind: "session" as const,
        label: `Session #${session.id} · ${lookupAgentName(session.agentId)}`,
        meta: `${toTitleCase(session.status)} · ${session.launchCommand} · ${formatRelativeTime(
          session.lastActivityAt ?? session.updatedAt
        )}`,
        sessionId: session.id
      }));

    const alerts = unresolvedAlerts
      .filter((alert) => {
        if (!query) return true;
        const agentName = lookupAgentName(alert.agentId);
        const displayMessage = displayAlertMessage(alert);
        const haystack = `${agentName} ${alert.reason} ${displayMessage} ${alert.sessionId}`.toLowerCase();
        return haystack.includes(query);
      })
      .slice(0, 20)
      .map((alert) => ({
        id: `alert-${alert.id}`,
        kind: "alert" as const,
        label: `${lookupAgentName(alert.agentId)} · ${toTitleCase(alert.reason)}`,
        meta: `Session #${alert.sessionId} · ${displayAlertMessage(alert)}`,
        alert
      }));

    return [...commands, ...sessionEntries, ...alerts];
  });

  $effect(() => {
    const maxIndex = Math.max(0, paletteEntries.length - 1);
    if (paletteSelectedIndex > maxIndex) {
      paletteSelectedIndex = maxIndex;
    }
  });

  async function loadUnresolvedAlerts() {
    unresolvedAlertsLoading = true;
    try {
      const alerts = (await invoke("list_session_alerts_cmd", {
        agentId: null,
        unresolvedOnly: true,
        limit: 200
      })) as SessionAlert[];
      unresolvedAlerts = alerts.map((alert) => ({
        ...alert,
        message: sanitizeDisplayText(alert.message, UI_SNIPPET_MAX_CHARS),
        messageEnriched: sanitizeOptionalText(alert.messageEnriched ?? null, UI_SNIPPET_MAX_CHARS)
      }));
    } catch (error) {
      console.error("Failed to load unresolved alerts", error);
      unresolvedAlerts = [];
    } finally {
      unresolvedAlertsLoading = false;
    }
  }

  function dismissAlertToast(toastId: string) {
    alertToasts = alertToasts.filter((toast) => toast.id !== toastId);
    const timer = alertToastTimers.get(toastId);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      alertToastTimers.delete(toastId);
    }
  }

  function upsertUnresolvedAlertFromEvent(payload: SessionAlertCreatedEvent) {
    const mapped: SessionAlert = {
      id: payload.alertId,
      sessionId: payload.sessionId,
      agentId: payload.agentId ?? null,
      severity: payload.severity,
      reason: payload.reason,
      message: sanitizeDisplayText(payload.message, UI_SNIPPET_MAX_CHARS),
      messageEnriched: sanitizeOptionalText(payload.messageEnriched ?? null, UI_SNIPPET_MAX_CHARS),
      messageEnrichmentStatus: payload.messageEnrichmentStatus ?? "pending",
      messageEnrichedAt: payload.messageEnrichedAt ?? null,
      messageEnrichmentError: null,
      requiresAck: payload.requiresAck,
      acknowledgedAt: null,
      snoozedUntil: null,
      escalatedAt: null,
      escalationCount: 0,
      resolvedAt: null,
      createdAt: payload.createdAt,
      updatedAt: payload.createdAt
    };
    unresolvedAlerts = [
      mapped,
      ...unresolvedAlerts.filter((entry) => entry.id !== mapped.id)
    ].slice(0, 200);
  }

  function removeUnresolvedAlertById(alertId: number) {
    for (const toast of alertToasts) {
      if (toast.alertId === alertId) {
        dismissAlertToast(toast.id);
      }
    }
    unresolvedAlerts = unresolvedAlerts.filter((entry) => entry.id !== alertId);
  }

  function applyEscalatedAlert(payload: SessionAlertEscalatedEvent) {
    let found = false;
    unresolvedAlerts = unresolvedAlerts.map((entry) => {
      if (entry.id !== payload.alertId) return entry;
      found = true;
      return {
        ...entry,
        severity: payload.severity,
        escalatedAt: payload.escalatedAt ?? entry.escalatedAt,
        escalationCount: payload.escalationCount ?? entry.escalationCount ?? 0,
        snoozedUntil: null
      };
    });
    if (!found) {
      void loadUnresolvedAlerts();
    }
  }

  function enqueueAlertToast(payload: SessionAlertCreatedEvent) {
    const toastId = `${payload.alertId}-${Date.now()}`;
    const displayMessage = sanitizeDisplayText(
      payload.messageEnriched ?? payload.message,
      UI_SNIPPET_MAX_CHARS
    );
    const toast: AlertToast = {
      id: toastId,
      alertId: payload.alertId,
      sessionId: payload.sessionId,
      agentId: payload.agentId ?? null,
      severity: payload.severity,
      reason: payload.reason,
      message: displayMessage
    };

    const nextToasts = [toast, ...alertToasts];
    const dropped = nextToasts.slice(ALERT_TOAST_MAX);
    alertToasts = nextToasts.slice(0, ALERT_TOAST_MAX);
    const timer = window.setTimeout(() => {
      dismissAlertToast(toastId);
    }, ALERT_TOAST_TTL_MS);
    alertToastTimers.set(toastId, timer);

    for (const entry of dropped) {
      dismissAlertToast(entry.id);
    }
  }

  function focusToastAlert(toast: AlertToast) {
    dismissAlertToast(toast.id);
    if (toast.agentId) {
      focusAgentById(toast.agentId, toast.sessionId);
      return;
    }
    void focusSession(toast.sessionId);
  }

  async function runAlertAction(
    action: "acknowledge" | "snooze" | "escalate",
    alertId: number
  ) {
    if (alertActionBusyId !== null) return;
    alertActionBusyId = alertId;
    try {
      if (action === "acknowledge") {
        await invoke("acknowledge_session_alert_cmd", { alertId });
      } else if (action === "snooze") {
        await invoke("snooze_session_alert_cmd", { alertId, durationMinutes: 30 });
      } else {
        await invoke("escalate_session_alert_cmd", { alertId });
      }
      await loadData();
    } catch (error) {
      console.error(`Failed to ${action} alert`, error);
    } finally {
      alertActionBusyId = null;
    }
  }

  async function runPaletteCommand(commandId: PaletteCommandId) {
    switch (commandId) {
      case "show-unresolved-inputs":
        await loadUnresolvedAlerts();
        paletteQuery = "";
        await tick();
        paletteSelectedIndex = Math.max(
          0,
          paletteEntries.findIndex((entry) => entry.kind === "alert")
        );
        break;
      case "voice-query-input-needed":
        closeCommandPalette();
        await runVoiceText("which agents need input");
        break;
      case "switch-mode-zen":
        closeCommandPalette();
        await invoke("set_app_mode_cmd", { mode: "zen" });
        break;
      case "switch-mode-full":
        closeCommandPalette();
        await invoke("set_app_mode_cmd", { mode: "full" });
        break;
      default:
        break;
    }
  }

  async function runPaletteEntry(entry?: PaletteEntry) {
    if (!entry) return;
    if (entry.kind === "command") {
      await runPaletteCommand(entry.commandId);
      return;
    }
    if (entry.kind === "session") {
      closeCommandPalette();
      await focusSession(entry.sessionId);
      return;
    }
    closeCommandPalette();
    if (entry.alert.agentId) {
      focusAgentById(entry.alert.agentId, entry.alert.sessionId);
      return;
    }
    await focusSession(entry.alert.sessionId);
  }

  async function openCommandPalette() {
    showCommandPalette = true;
    paletteQuery = "";
    paletteSelectedIndex = 0;
    await loadUnresolvedAlerts();
    await tick();
    paletteInput?.focus();
  }

  function closeCommandPalette() {
    showCommandPalette = false;
    paletteQuery = "";
    paletteSelectedIndex = 0;
  }

  async function handlePaletteInputKeydown(event: KeyboardEvent) {
    if (!showCommandPalette) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      paletteSelectedIndex = Math.min(paletteSelectedIndex + 1, Math.max(0, paletteEntries.length - 1));
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      paletteSelectedIndex = Math.max(paletteSelectedIndex - 1, 0);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      await runPaletteEntry(paletteEntries[paletteSelectedIndex]);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeCommandPalette();
    }
  }

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
      terminalViewportState.selectedSessionId = null;
      persistSelectedSession(null);
      clearTerminalView();
      return;
    }

    if (attachedSessionId && attachedSessionId !== sessionId) {
      await detachTerminalSession(attachedSessionId);
    }

    selectedSessionId = sessionId;
    terminalViewportState.selectedSessionId = sessionId;
    persistSelectedSession(sessionId);

    if (attachedSessionId !== sessionId) {
      await attachTerminalSession(sessionId);
    }

    fitAddon?.fit();
    await resizeTerminalSession(sessionId);

    const hasSavedCursor = terminalCursorBySession.has(sessionId);
    if (reset || !hasSavedCursor) {
      await hydrateTerminalSession(sessionId, { reset: true });
      return;
    }
    await streamTerminalChunks(sessionId);
  }

  async function openTerminalPopout() {
    if (!selectedSessionId) return;
    const sessionId = selectedSessionId;
    const popoutUrl = `/terminal-popout?sessionId=${sessionId}`;

    try {
      const existing = await WebviewWindow.getByLabel(TERMINAL_POPOUT_WINDOW_LABEL);
      if (existing) {
        await existing.emit("terminal-popout-focus-session", { sessionId });
        return;
      }

      const popout = new WebviewWindow(TERMINAL_POPOUT_WINDOW_LABEL, {
        title: `Terminal Session #${sessionId}`,
        url: popoutUrl,
        width: 1180,
        height: 760,
        minWidth: 860,
        minHeight: 520,
        resizable: true,
        focus: true
      });

      popout.once("tauri://error", (event) => {
        console.error("Failed to create terminal popout window", event.payload);
      });
    } catch (error) {
      console.error("Terminal popout unavailable in this context", error);
      window.open(popoutUrl, "_blank", "noopener,noreferrer");
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
    let cursorUpdated = false;
    while (queue.length > 0 && writtenBytes < TERMINAL_FRAME_WRITE_BUDGET_BYTES) {
      const next = queue.shift();
      if (!next) break;
      terminalWidget.write(next.chunk);
      terminalCursorBySession.set(selectedSessionId, next.cursor);
      writtenBytes += next.chunk.length;
      cursorUpdated = true;
    }

    if (cursorUpdated) {
      schedulePersistTerminalCursors();
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
      schedulePersistTerminalCursors();
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
      schedulePersistTerminalCursors();
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
      await loadData({ background: true });
    } catch (error) {
      console.error("Failed to send terminal input", error);
    }
  }

  async function runVoiceText(text: string, options: { confirmed?: boolean } = {}) {
    const { confirmed = false } = options;
    const transcript = confirmed ? `confirm ${text}` : text;

    if (!text) return;

    try {
      await invoke("process_voice_text_cmd", { text: transcript });
      await clearRuntimeIssue("model_down");
    } catch (error) {
      console.error("Failed to process voice text", error);
      await reportRuntimeIssue({ error, source: "voice" });
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
      await clearRuntimeIssue("mic_unavailable");
      await clearRuntimeIssue("model_down");
    } catch (error) {
      console.error("Failed to run push-to-talk command", error);
      await reportRuntimeIssue({ error, source: "voice" });
    } finally {
      pushToTalkBusy = false;
      invoke("voice_status_cmd")
        .then((status) => {
          const typed = status as VoiceStatus;
          voiceState = typed.state;
          lastTranscript = sanitizeDisplayText(
            typed.lastTranscript ?? lastTranscript,
            UI_VOICE_LINE_MAX_CHARS
          );
        })
        .catch((error) => {
          console.error("Failed to refresh voice status", error);
          void reportRuntimeIssue({ error, source: "voice" });
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
        scheduleInputRequestRefresh();
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

  function handleGlobalKeydown(event: KeyboardEvent) {
    const lowered = event.key.toLowerCase();
    if ((event.metaKey || event.ctrlKey) && lowered === "k") {
      event.preventDefault();
      if (showCommandPalette) {
        closeCommandPalette();
      } else {
        void openCommandPalette();
      }
      return;
    }
    if (event.key === "Escape" && showCommandPalette) {
      event.preventDefault();
      closeCommandPalette();
    }
  }

  async function registerUiListeners() {
    clearUiListeners();
    try {
      uiUnlisteners = await Promise.all([
        listen("task_updated", () => {
          void loadData();
        }),
        listen("agent_updated", () => {
          void loadData();
        }),
        listen("terminal_chunk", (event) => {
          const payload = event.payload as TerminalChunkEvent;
          queueTerminalChunk(payload);
        }),
        listen("agent_runtime_updated", (event) => {
          const payload = event.payload as AgentRuntimeUpdatedEvent;
          const selectedSession = selectedSessionId
            ? sessions.find((session) => session.id === selectedSessionId)
            : undefined;
          const selectedSessionIsActiveForAgent =
            !!selectedSession &&
            selectedSession.agentId === payload.agentId &&
            isActiveSessionStatus(selectedSession.status);
          if (
            payload.activeSessionId &&
            selectedAgentId === payload.agentId &&
            selectedSessionId !== payload.activeSessionId &&
            !selectedSessionIsActiveForAgent
          ) {
            void setSelectedSession(payload.activeSessionId, { reset: false });
          }
          void loadData({ background: true });
        }),
        listen("session_alert_created", (event) => {
          const payload = event.payload as SessionAlertCreatedEvent;
          upsertUnresolvedAlertFromEvent(payload);
          enqueueAlertToast(payload);
          void reportRuntimeIssue({
            error: payload.messageEnriched ?? payload.message,
            source: "session_alert"
          });
        }),
        listen("session_alert_resolved", (event) => {
          const payload = event.payload as SessionAlertResolvedEvent;
          removeUnresolvedAlertById(payload.alertId);
          void loadData({ background: true });
        }),
        listen("session_alert_snoozed", (event) => {
          const payload = event.payload as SessionAlertSnoozedEvent;
          removeUnresolvedAlertById(payload.alertId);
          void loadData({ background: true });
        }),
        listen("session_alert_escalated", (event) => {
          const payload = event.payload as SessionAlertEscalatedEvent;
          applyEscalatedAlert(payload);
          void loadData({ background: true });
        }),
        listen("managed_session_prompt_required", async (event) => {
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
            await runVoiceText(candidate, { confirmed: true });
          }
        }),
        listen("voice_state_updated", (event) => {
          const payload = event.payload as { state: string };
          voiceState = payload.state;
        }),
        listen("voice_transcript", (event) => {
          const payload = event.payload as { text: string };
          lastTranscript = sanitizeDisplayText(payload.text, UI_VOICE_LINE_MAX_CHARS);
          lastVoiceCommandText = payload.text;
        }),
        listen("voice_intent", (event) => {
          const payload = event.payload as VoiceIntentEvent;
          lastIntent = summarizeVoiceIntent(payload);
        }),
        listen("voice_action_executed", (event) => {
          const payload = event.payload as VoiceAction;
          const actionText = sanitizeDisplayText(payload.action, 40);
          const resultText = sanitizeDisplayText(payload.result, 24);
          const detail =
            typeof payload.text === "string" && payload.text.trim()
              ? `: ${sanitizeDisplayText(payload.text, 90)}`
              : "";
          lastCommand = sanitizeDisplayText(
            `${actionText} (${resultText})${detail}`,
            UI_VOICE_LINE_MAX_CHARS
          );
          void loadData({ background: true });
        }),
        listen("voice_error", (event) => {
          const payload = event.payload as { message: string };
          lastCommand = sanitizeDisplayText(
            `error: ${payload.message}`,
            UI_VOICE_LINE_MAX_CHARS
          );
          void reportRuntimeIssue({ error: payload.message, source: "voice" });
        }),
        listen("voice_status_reply", (event) => {
          const payload = event.payload as VoiceStatusReplyEvent;
          lastCommand = sanitizeDisplayText(payload.summary, UI_VOICE_LINE_MAX_CHARS);
        })
      ]);
      listenerReconnectAttempts = 0;
      return true;
    } catch (error) {
      console.error("Failed to register UI listeners", error);
      await reportRuntimeIssue({ error, source: "system" });
      clearUiListeners();
      return false;
    }
  }

  function scheduleUiListenerReconnect(reason: string) {
    if (listenerLifecycleStopped || listenerReconnectTimer) return;
    listenerReconnectAttempts += 1;
    const delayMs = listenerReconnectDelayMs(listenerReconnectAttempts);
    listenerReconnectTimer = window.setTimeout(() => {
      listenerReconnectTimer = 0;
      void registerUiListeners().then((connected) => {
        if (!connected) {
          scheduleUiListenerReconnect("listener reconnect retry");
          return;
        }
        void loadData({ background: true });
        if (selectedSessionId) {
          void resyncTerminalStream(selectedSessionId);
        }
      });
    }, delayMs);
    console.warn(
      `Scheduling UI listener reconnect in ${delayMs}ms (reason: ${reason}, attempt: ${listenerReconnectAttempts})`
    );
  }

  function handleWindowResume() {
    if (document.visibilityState === "hidden") return;
    if (uiUnlisteners.length === 0) {
      scheduleUiListenerReconnect("window resume with no listeners");
      return;
    }
    void loadData({ background: true });
    if (selectedSessionId) {
      void resyncTerminalStream(selectedSessionId);
    }
  }

  onMount(() => {
    listenerLifecycleStopped = false;
    restoreTerminalViewportState();
    terminalViewportState.selectedSessionId = selectedSessionId;
    void initTerminalWidget();

    void registerUiListeners().then((connected) => {
      if (!connected) {
        scheduleUiListenerReconnect("initial listener registration failed");
      }
    });

    invoke("voice_status_cmd")
      .then((status) => {
        const typed = status as VoiceStatus;
        voiceState = typed.state;
        lastTranscript = sanitizeDisplayText(
          typed.lastTranscript ?? "",
          UI_VOICE_LINE_MAX_CHARS
        );
      })
      .catch((error) => {
        console.error("Failed to get voice status", error);
        void reportRuntimeIssue({ error, source: "voice" });
      });

    const interval = setInterval(() => {
      void loadData({ background: true });
      if (selectedSessionId) {
        void streamTerminalChunks(selectedSessionId);
      }
    }, 8000);
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
    window.addEventListener("keydown", handleGlobalKeydown);
    window.addEventListener("focus", handleWindowResume);
    document.addEventListener("visibilitychange", handleWindowResume);
    window.addEventListener("beforeunload", persistTerminalCursors);

    return () => {
      listenerLifecycleStopped = true;
      clearInterval(interval);
      resizeObserver?.disconnect();
      window.removeEventListener("keydown", handleGlobalKeydown);
      window.removeEventListener("focus", handleWindowResume);
      document.removeEventListener("visibilitychange", handleWindowResume);
      window.removeEventListener("beforeunload", persistTerminalCursors);
      if (listenerReconnectTimer) {
        window.clearTimeout(listenerReconnectTimer);
        listenerReconnectTimer = 0;
      }
      if (terminalCursorPersistTimer) {
        window.clearTimeout(terminalCursorPersistTimer);
        terminalCursorPersistTimer = 0;
      }
      if (inputRequestRefreshTimer) {
        window.clearTimeout(inputRequestRefreshTimer);
        inputRequestRefreshTimer = 0;
      }
      persistTerminalCursors();
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
      clearUiListeners();
      for (const timer of alertToastTimers.values()) {
        window.clearTimeout(timer);
      }
      alertToastTimers.clear();
    };
  });
</script>

<main class="app-shell">
  {#if alertToasts.length > 0}
    <section class="alert-toast-stack" aria-live="polite" aria-label="Input required alerts">
      {#each alertToasts as toast (toast.id)}
        <article class={`alert-toast severity-${toast.severity.toLowerCase()}`}>
          <button class="alert-toast-open" onclick={() => focusToastAlert(toast)}>
            <strong>{lookupAgentName(toast.agentId)} · {toTitleCase(toast.reason)}</strong>
            <p>{toast.message}</p>
            <span>Session #{toast.sessionId}</span>
          </button>
          <button class="ghost compact" onclick={() => dismissAlertToast(toast.id)}>Dismiss</button>
        </article>
      {/each}
    </section>
  {/if}

  {#if activeRuntimeIssues.length > 0}
    <section class="runtime-issues-panel" aria-live="polite" aria-label="Runtime issues">
      <header class="runtime-issues-header">
        <h2>System issues</h2>
        <span>{activeRuntimeIssues.length}</span>
      </header>
      <div class="runtime-issues-list">
        {#each activeRuntimeIssues as issue (issue.kind)}
          <article class={`runtime-issue severity-${issue.severity}`}>
            <div class="runtime-issue-copy">
              <strong>{issue.title}</strong>
              <p>{issue.message}</p>
              <p>{issue.guidance}</p>
              <span>
                Source: {issue.source} · Seen {formatRelativeTime(issue.lastSeenAt)} ·
                Count {issue.count}
              </span>
            </div>
            <button class="ghost compact" onclick={() => void dismissRuntimeIssue(issue.kind)}
              >Dismiss</button
            >
          </article>
        {/each}
      </div>
    </section>
  {/if}

  <section class="workspace">
    <section class="terminals-pane">
      <header class="pane-header terminal-header">
        <div class="terminal-title-block">
          <h1>Terminal</h1>
          <p>
            {#if selectedSession}
              Session #{selectedSession.id} · {lookupAgentName(selectedSession.agentId)}
            {:else}
              No session selected
            {/if}
          </p>
        </div>
        <div class="pane-header-actions">
          <button class="ghost compact" onclick={() => void openCommandPalette()}>
            Sessions (Cmd/Ctrl+K)
          </button>
          <button
            class="primary"
            onclick={() => startSession(selectedAgent?.id, selectedAgent?.taskId ?? undefined)}
            disabled={!selectedAgent}
          >
            New session
          </button>
        </div>
      </header>

      <div class="current-agent-strip" aria-label="Current agent">
        {#if selectedAgent}
          <strong>{selectedAgent.name}</strong>
          <span>{selectedAgent.provider}</span>
          <span class={`status ${selectedAgent.activeSessionStatus ?? selectedAgent.state}`}>
            {toTitleCase(selectedAgent.activeSessionStatus ?? selectedAgent.state)}
          </span>
          <span>
            Task: {selectedAgent.taskTitle ?? "Unassigned"}
          </span>
        {:else}
          <span>No focused agent</span>
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
          <div class="terminal-view-actions">
            {#if selectedSession}
              <button class="ghost compact" onclick={openTerminalPopout}>Pop out</button>
              <span class={`status ${selectedSession.status}`}>{toTitleCase(selectedSession.status)}</span>
            {/if}
          </div>
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
      <section class="activity-split-panel" aria-label="Open sessions">
        <header class="split-panel-header">
          <h3>Open sessions</h3>
          <span>{openAgentSessions.length}</span>
        </header>
        <div class="split-panel-scroll">
          {#if openAgentSessions.length === 0}
            <p class="empty-state">No open sessions.</p>
          {:else}
            {#each openAgentSessions as session (session.id)}
              <article class="open-agent-item">
                <button
                  class="open-agent-open"
                  onclick={() => void focusSession(session.id)}
                >
                  <strong>{lookupAgentName(session.agentId)}</strong>
                  <p>{session.provider} · {toTitleCase(session.status)}</p>
                  <p>Task: {lookupAgentTaskTitle(session.agentId)}</p>
                  <p>Session #{session.id}</p>
                  <p>Last activity: {formatRelativeTime(session.lastActivityAt ?? session.updatedAt)}</p>
                </button>
              </article>
            {/each}
          {/if}
        </div>
      </section>

      <section class="activity-split-panel" aria-label="Input requests">
        <header class="split-panel-header">
          <h3>Input requests</h3>
          <span>{visibleInputRequests.length}</span>
        </header>
        <div class="split-panel-scroll">
          {#if unresolvedAlertsLoading && visibleInputRequests.length === 0}
            <p class="empty-state">Loading input requests...</p>
          {:else if visibleInputRequests.length === 0}
            <p class="empty-state">No unresolved input requests.</p>
          {:else}
            {#each visibleInputRequests as alert (alert.id)}
              <article class={`alert-item severity-${alert.severity.toLowerCase()}`}>
                <button
                  class="alert-open"
                  onclick={() => {
                    if (alert.agentId) {
                      focusAgentById(alert.agentId, alert.sessionId);
                    } else {
                      void focusSession(alert.sessionId);
                    }
                  }}
                >
                  <strong>{lookupAgentName(alert.agentId)} · {toTitleCase(alert.reason)}</strong>
                  <p>Session #{alert.sessionId} · {toTitleCase(alert.severity)}</p>
                  <p>{displayAlertMessage(alert)}</p>
                </button>
                <div class="alert-item-actions">
                  <button
                    class="ghost compact"
                    onclick={() => void runAlertAction("acknowledge", alert.id)}
                    disabled={alertActionBusyId !== null || !!alert.acknowledgedAt}
                  >
                    {alert.acknowledgedAt ? "Acked" : "Acknowledge"}
                  </button>
                  <button class="ghost compact" onclick={() => void openCommandPalette()}>
                    Open palette
                  </button>
                </div>
              </article>
            {/each}
          {/if}
        </div>
      </section>
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
      <button class="ghost" onclick={() => void openCommandPalette()}>Palette (Cmd/Ctrl+K)</button>
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

  {#if showCommandPalette}
    <div
      class="command-palette-backdrop"
      role="presentation"
      onclick={(event) => {
        if (event.target === event.currentTarget) {
          closeCommandPalette();
        }
      }}
    >
      <dialog open class="command-palette" aria-label="Command palette">
        <header class="command-palette-header">
          <input
            bind:this={paletteInput}
            bind:value={paletteQuery}
            placeholder="Search commands, sessions, or alerts"
            oninput={() => {
              paletteSelectedIndex = 0;
            }}
            onkeydown={handlePaletteInputKeydown}
          />
          <button class="ghost compact" onclick={closeCommandPalette}>Close</button>
        </header>
        <p class="command-palette-hint">Enter to run. Arrow keys to navigate.</p>
        <div class="command-palette-list">
          {#if unresolvedAlertsLoading}
            <p class="empty-state">Loading unresolved alerts...</p>
          {:else if paletteEntries.length === 0}
            <p class="empty-state">No matching commands or alerts.</p>
          {:else}
            {#each paletteEntries as entry, index (entry.id)}
              <button
                class="command-palette-item"
                class:selected={index === paletteSelectedIndex}
                onclick={() => void runPaletteEntry(entry)}
                onmousemove={() => (paletteSelectedIndex = index)}
              >
                <div class="command-palette-copy">
                  <strong>{entry.label}</strong>
                  <p>{entry.meta}</p>
                </div>
                <span class="command-palette-kind">
                  {entry.kind === "command" ? "Command" : entry.kind === "session" ? "Session" : "Alert"}
                </span>
              </button>
            {/each}
          {/if}
        </div>
      </dialog>
    </div>
  {/if}

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
    background: #07080a;
    color: #d6dde6;
    font-family: "JetBrains Mono", "SFMono-Regular", "Menlo", monospace;
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
    background-color: #07080a;
    background-image: linear-gradient(rgba(145, 168, 190, 0.05) 1px, transparent 1px),
      linear-gradient(90deg, rgba(145, 168, 190, 0.03) 1px, transparent 1px);
    background-size: 100% 3px, 3px 100%;
  }

  .alert-toast-stack {
    position: fixed;
    z-index: 60;
    top: 16px;
    right: 16px;
    width: min(360px, calc(100vw - 32px));
    display: grid;
    gap: 8px;
  }

  .alert-toast {
    border-radius: 6px;
    border: 1px solid rgba(118, 138, 162, 0.38);
    background: rgba(7, 10, 15, 0.96);
    box-shadow: 0 14px 28px rgba(0, 0, 0, 0.35);
    padding: 10px;
    display: grid;
    gap: 8px;
  }

  .alert-toast.severity-warning {
    border-color: rgba(255, 184, 92, 0.64);
  }

  .alert-toast.severity-critical {
    border-color: rgba(255, 123, 114, 0.72);
    background: rgba(35, 13, 14, 0.95);
  }

  .alert-toast-open {
    width: 100%;
    border: 0;
    border-radius: 8px;
    padding: 0;
    margin: 0;
    background: transparent;
    color: inherit;
    text-align: left;
    display: grid;
    gap: 4px;
    cursor: pointer;
  }

  .alert-toast-open strong,
  .alert-toast-open p,
  .alert-toast-open span {
    margin: 0;
  }

  .alert-toast-open strong {
    font-size: 12px;
  }

  .alert-toast-open p,
  .alert-toast-open span {
    font-size: 11px;
    color: rgba(221, 231, 239, 0.84);
  }

  .runtime-issues-panel {
    border: 1px solid rgba(255, 123, 114, 0.46);
    border-radius: 6px;
    background: rgba(32, 14, 16, 0.88);
    padding: 10px;
    display: grid;
    gap: 8px;
  }

  .runtime-issues-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .runtime-issues-header h2 {
    margin: 0;
    font-size: 12px;
    letter-spacing: 0.07em;
    text-transform: uppercase;
  }

  .runtime-issues-header span {
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: rgba(255, 208, 205, 0.85);
  }

  .runtime-issues-list {
    display: grid;
    gap: 7px;
  }

  .runtime-issue {
    border-radius: 6px;
    border: 1px solid rgba(255, 123, 114, 0.5);
    background: rgba(43, 14, 17, 0.74);
    padding: 8px;
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 8px;
  }

  .runtime-issue.severity-warning {
    border-color: rgba(255, 184, 92, 0.55);
    background: rgba(45, 30, 14, 0.7);
  }

  .runtime-issue-copy {
    min-width: 0;
    display: grid;
    gap: 4px;
  }

  .runtime-issue-copy strong,
  .runtime-issue-copy p,
  .runtime-issue-copy span {
    margin: 0;
  }

  .runtime-issue-copy strong {
    font-size: 12px;
  }

  .runtime-issue-copy p {
    font-size: 11px;
    color: rgba(237, 223, 222, 0.92);
  }

  .runtime-issue-copy span {
    font-size: 10px;
    color: rgba(229, 204, 203, 0.85);
  }

  .workspace {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(260px, 320px);
    gap: 12px;
  }

  .terminals-pane,
  .activity-pane {
    min-height: 0;
    background: rgba(5, 8, 12, 0.95);
    border: 1px solid rgba(118, 138, 162, 0.32);
    border-radius: 8px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.02);
  }

  .activity-pane {
    display: grid;
    grid-template-rows: minmax(0, 1fr) minmax(0, 1fr);
    gap: 12px;
  }

  .pane-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .pane-header h1 {
    margin: 0;
    font-size: 13px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: rgba(214, 221, 230, 0.92);
  }

  .pane-header span {
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: rgba(214, 221, 230, 0.62);
  }

  .pane-header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .terminal-header {
    align-items: flex-start;
  }

  .terminal-title-block {
    display: grid;
    gap: 4px;
  }

  .terminal-title-block p {
    margin: 0;
    font-size: 11px;
    color: rgba(214, 221, 230, 0.66);
  }

  .current-agent-strip {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    font-size: 11px;
    padding: 8px 10px;
    border: 1px solid rgba(118, 138, 162, 0.3);
    border-radius: 6px;
    background: rgba(8, 11, 16, 0.9);
    color: rgba(214, 221, 230, 0.75);
  }

  .current-agent-strip strong {
    color: rgba(229, 236, 245, 0.96);
  }

  .current-agent-strip .status {
    border: 1px solid rgba(118, 138, 162, 0.38);
    border-radius: 4px;
    padding: 1px 6px;
    background: rgba(16, 20, 29, 0.85);
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
    font-size: 12px;
    color: rgba(214, 221, 230, 0.82);
  }

  .terminal-view-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .terminal-widget {
    flex: 1;
    min-height: 420px;
    overflow: hidden;
    background: #020304;
    border: 1px solid rgba(118, 138, 162, 0.35);
    border-radius: 8px;
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

  .activity-split-panel {
    border: 1px solid rgba(118, 138, 162, 0.3);
    border-radius: 6px;
    background: rgba(8, 11, 16, 0.9);
    padding: 10px;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .split-panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .split-panel-header h3 {
    margin: 0;
    font-size: 12px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .split-panel-header span {
    font-size: 11px;
    color: rgba(214, 221, 230, 0.7);
  }

  .split-panel-scroll {
    min-height: 0;
    overflow-y: auto;
    display: grid;
    gap: 8px;
    padding-right: 4px;
  }

  .open-agent-item {
    border-radius: 6px;
    border: 1px solid rgba(118, 138, 162, 0.32);
    background: rgba(7, 10, 15, 0.9);
    padding: 8px;
  }

  .open-agent-open {
    width: 100%;
    border: 0;
    border-radius: 8px;
    padding: 0;
    margin: 0;
    text-align: left;
    background: transparent;
    color: inherit;
    cursor: pointer;
    transform: none;
    display: grid;
    gap: 4px;
  }

  .open-agent-open:not(:disabled):hover {
    transform: none;
  }

  .open-agent-open strong,
  .open-agent-open p {
    margin: 0;
  }

  .open-agent-open strong {
    font-size: 12px;
    line-height: 1.3;
  }

  .open-agent-open p {
    font-size: 11px;
    color: rgba(221, 231, 239, 0.77);
    line-height: 1.35;
  }

  .terminal-input-row input,
  .voice-input input {
    border: 1px solid rgba(118, 138, 162, 0.42);
    border-radius: 6px;
    padding: 10px 12px;
    background: rgba(5, 8, 12, 0.92);
    color: inherit;
  }

  .alert-item {
    border-radius: 6px;
    border: 1px solid rgba(118, 138, 162, 0.32);
    background: rgba(7, 10, 15, 0.9);
    padding: 8px;
    display: grid;
    gap: 8px;
  }

  .alert-item.severity-warning {
    border-color: rgba(255, 184, 92, 0.5);
  }

  .alert-item.severity-critical {
    border-color: rgba(255, 123, 114, 0.58);
    background: rgba(34, 16, 16, 0.82);
  }

  .alert-open {
    width: 100%;
    border: 0;
    border-radius: 8px;
    padding: 0;
    margin: 0;
    text-align: left;
    background: transparent;
    color: inherit;
    cursor: pointer;
    transform: none;
    display: grid;
    gap: 4px;
  }

  .alert-open:not(:disabled):hover {
    transform: none;
  }

  .alert-open strong,
  .alert-open p {
    margin: 0;
  }

  .alert-open strong {
    font-size: 12px;
    line-height: 1.3;
  }

  .alert-open p {
    font-size: 11px;
    color: rgba(221, 231, 239, 0.77);
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .alert-item-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .command-palette-backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: rgba(2, 5, 10, 0.68);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 10vh 16px 24px;
  }

  .command-palette {
    width: min(760px, 100%);
    max-height: min(68vh, 640px);
    display: flex;
    flex-direction: column;
    gap: 8px;
    border-radius: 8px;
    border: 1px solid rgba(118, 138, 162, 0.42);
    background: rgba(7, 10, 15, 0.98);
    color: #d6dde6;
    box-shadow: 0 20px 34px rgba(0, 0, 0, 0.5);
    padding: 10px;
  }

  .command-palette-header {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 8px;
  }

  .command-palette-header input {
    border: 1px solid rgba(118, 138, 162, 0.55);
    border-radius: 6px;
    padding: 10px 12px;
    background: rgba(5, 8, 12, 0.95);
    color: inherit;
    min-width: 0;
  }

  .command-palette-hint {
    margin: 0;
    padding: 0 2px;
    font-size: 11px;
    color: rgba(221, 231, 239, 0.72);
  }

  .command-palette-list {
    min-height: 0;
    overflow-y: auto;
    display: grid;
    gap: 6px;
    padding-right: 4px;
  }

  .command-palette-item {
    width: 100%;
    border: 1px solid rgba(118, 138, 162, 0.32);
    background: rgba(8, 11, 16, 0.86);
    border-radius: 6px;
    padding: 10px;
    color: inherit;
    text-align: left;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .command-palette-item.selected {
    border-color: rgba(91, 213, 204, 0.8);
    background: rgba(13, 25, 30, 0.9);
  }

  .command-palette-copy {
    min-width: 0;
    display: grid;
    gap: 2px;
  }

  .command-palette-copy strong,
  .command-palette-copy p {
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .command-palette-copy strong {
    font-size: 12px;
  }

  .command-palette-copy p {
    font-size: 11px;
    color: rgba(221, 231, 239, 0.72);
  }

  .command-palette-kind {
    border: 1px solid rgba(118, 138, 162, 0.45);
    border-radius: 4px;
    font-size: 10px;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 2px 8px;
    color: rgba(225, 236, 245, 0.85);
    background: rgba(8, 11, 16, 0.92);
    flex-shrink: 0;
  }

  .voice-toolbar {
    display: grid;
    grid-template-columns: 1.3fr auto 1fr;
    gap: 10px;
    align-items: center;
    background: rgba(5, 8, 12, 0.95);
    border: 1px solid rgba(118, 138, 162, 0.35);
    border-radius: 8px;
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
    border: 1px solid rgba(118, 138, 162, 0.4);
    border-radius: 6px;
    padding: 8px 12px;
    font-weight: 600;
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
    background: rgba(18, 24, 34, 0.92);
    color: #dbe3eb;
  }

  button.compact {
    padding: 6px 10px;
    font-size: 11px;
  }

  .primary {
    background: linear-gradient(180deg, #82dd78 0%, #5abf74 100%);
    color: #041a08;
    border-color: rgba(96, 192, 116, 0.6);
  }

  .status {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .status.active {
    color: #71d5c9;
  }

  .status.waking,
  .status.stalled {
    color: #d9b15f;
  }

  .status.needs_input,
  .status.failed {
    color: #d98781;
  }

  .status.ended {
    color: rgba(221, 231, 239, 0.56);
  }

  .status.idle {
    color: rgba(221, 231, 239, 0.56);
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

  }
</style>
