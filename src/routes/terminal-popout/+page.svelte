<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, tick } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";

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

  type FocusSessionEvent = {
    sessionId?: number | null;
  };

  type BufferedTerminalChunk = {
    chunk: string;
    cursor: number;
  };

  let loading = $state<boolean>(true);
  let hasLoadedOnce = $state<boolean>(false);
  let sessions = $state<SessionRuntime[]>([]);
  let selectedSessionId = $state<number | null>(null);
  let attachedSessionId = $state<number | null>(null);
  let terminalInput = $state<string>("");

  let terminalContainer: HTMLDivElement | null = $state(null);

  let terminalWidget: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  let terminalDataListener: { dispose: () => void } | null = null;
  let terminalFlushRaf = 0;

  const terminalCursorBySession = new Map<number, number>();
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

  const selectedSession = $derived(
    selectedSessionId ? sessions.find((session) => session.id === selectedSessionId) : undefined
  );

  const activeSessions = $derived(
    sessions.filter((session) => ACTIVE_SESSION_STATUSES.has(session.status))
  );

  function parseSessionId(raw: string | null): number | null {
    if (!raw) return null;
    const parsed = Number.parseInt(raw, 10);
    if (Number.isNaN(parsed) || parsed <= 0) return null;
    return parsed;
  }

  function readSessionIdFromUrl() {
    return parseSessionId(new URLSearchParams(window.location.search).get("sessionId"));
  }

  const toTitleCase = (value: string) =>
    value
      .split(/[\s_-]+/)
      .filter(Boolean)
      .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
      .join(" ");

  function formatRelativeTime(value: string | null | undefined) {
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
  }

  function updateSessionInUrl(sessionId: number | null) {
    const url = new URL(window.location.href);
    if (sessionId) {
      url.searchParams.set("sessionId", String(sessionId));
    } else {
      url.searchParams.delete("sessionId");
    }
    window.history.replaceState({}, "", url.toString());
  }

  function upsertSessionRuntime(session: SessionRuntime) {
    const existing = sessions.find((entry) => entry.id === session.id);
    if (existing) {
      sessions = sessions.map((entry) => (entry.id === session.id ? session : entry));
      return;
    }
    sessions = [session, ...sessions];
  }

  async function loadSessions(options: { background?: boolean } = {}) {
    const background = options.background ?? hasLoadedOnce;
    if (!background) {
      loading = true;
    }
    try {
      const rows = (await invoke("list_managed_sessions_cmd", {
        status: null,
        limit: 200
      })) as SessionRuntime[];
      sessions = rows;

      const requested = readSessionIdFromUrl();
      let nextSessionId = selectedSessionId;
      if (nextSessionId && !sessions.some((session) => session.id === nextSessionId)) {
        nextSessionId = null;
      }
      if (!nextSessionId && requested && sessions.some((session) => session.id === requested)) {
        nextSessionId = requested;
      }
      if (!nextSessionId) {
        nextSessionId =
          sessions.find((session) => ACTIVE_SESSION_STATUSES.has(session.status))?.id ??
          sessions[0]?.id ??
          null;
      }

      if (nextSessionId !== selectedSessionId) {
        await setSelectedSession(nextSessionId, { reset: true });
      } else if (nextSessionId === null) {
        clearTerminalView();
      }
      hasLoadedOnce = true;
    } catch (error) {
      console.error("Failed to load sessions for popout terminal", error);
    } finally {
      if (!background) {
        loading = false;
      }
    }
  }

  async function attachTerminalSession(sessionId: number) {
    try {
      const updated = (await invoke("attach_terminal_session_cmd", { sessionId })) as SessionRuntime;
      upsertSessionRuntime(updated);
      attachedSessionId = sessionId;
    } catch (error) {
      console.error("Failed to attach popout terminal session", error);
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
      console.error("Failed to detach popout terminal session", error);
    }
  }

  async function setSelectedSession(sessionId: number | null, options: { reset?: boolean } = {}) {
    const { reset = true } = options;

    if (sessionId === null) {
      if (attachedSessionId) {
        await detachTerminalSession(attachedSessionId);
      }
      selectedSessionId = null;
      updateSessionInUrl(null);
      clearTerminalView();
      return;
    }

    if (attachedSessionId && attachedSessionId !== sessionId) {
      await detachTerminalSession(attachedSessionId);
    }

    selectedSessionId = sessionId;
    updateSessionInUrl(sessionId);

    if (attachedSessionId !== sessionId) {
      await attachTerminalSession(sessionId);
    }

    fitAddon?.fit();
    await resizeTerminalSession(sessionId);

    if (reset || terminalCursorBySession.get(sessionId) === undefined) {
      await hydrateTerminalSession(sessionId, { reset: true });
    }
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
      console.error("Failed to stream popout terminal output", error);
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
      console.error("Failed to send terminal input from popout", error);
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
        fontSize: 13,
        lineHeight: 1.35,
        theme: {
          background: "#050a11",
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
          console.error("Failed to send raw terminal data from popout", error);
        });
      });

      await tick();
      fitAddon.fit();
      clearTerminalView();
      if (selectedSessionId) {
        await resizeTerminalSession(selectedSessionId);
      }
    } catch (error) {
      console.error("Failed to initialize popout terminal widget", error);
      if (terminalContainer) {
        terminalContainer.textContent = "Terminal widget unavailable.";
      }
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
      console.error("Failed to resize popout terminal session", error);
    }
  }

  onMount(() => {
    let unlistenTerminal: (() => void) | undefined;
    let unlistenFocusSession: (() => void) | undefined;

    void initTerminalWidget();

    const startListeners = async () => {
      unlistenTerminal = await listen("terminal_chunk", (event) => {
        const payload = event.payload as TerminalChunkEvent;
        queueTerminalChunk(payload);
      });

      unlistenFocusSession = await listen("terminal-popout-focus-session", (event) => {
        const payload = event.payload as FocusSessionEvent;
        const targetSessionId =
          typeof payload?.sessionId === "number" && payload.sessionId > 0
            ? payload.sessionId
            : null;
        if (targetSessionId) {
          void setSelectedSession(targetSessionId, { reset: true });
        }
      });
    };

    void startListeners();

    const interval = setInterval(() => {
      void loadSessions({ background: true });
    }, 8000);
    void loadSessions();

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
      unlistenTerminal?.();
      unlistenFocusSession?.();
    };
  });
</script>

<main class="popout-shell">
  <header class="popout-header">
    <div>
      <h1>Terminal popout</h1>
      {#if selectedSession}
        <p>Session #{selectedSession.id} · {selectedSession.launchCommand}</p>
      {:else}
        <p>No session selected.</p>
      {/if}
    </div>

    <div class="header-actions">
      <label>
        Session
        <select
          value={selectedSessionId ?? ""}
          onchange={(event) => {
            const value = Number.parseInt((event.currentTarget as HTMLSelectElement).value, 10);
            void setSelectedSession(Number.isNaN(value) ? null : value, { reset: true });
          }}
        >
          <option value="">None</option>
          {#each activeSessions as session}
            <option value={session.id}>#{session.id} · {toTitleCase(session.status)}</option>
          {/each}
          {#if selectedSession && !activeSessions.some((session) => session.id === selectedSession.id)}
            <option value={selectedSession.id}>#{selectedSession.id} · {toTitleCase(selectedSession.status)}</option>
          {/if}
        </select>
      </label>
      {#if selectedSession}
        <span class={`status ${selectedSession.status}`}>{toTitleCase(selectedSession.status)}</span>
      {/if}
    </div>
  </header>

  <section class="terminal-panel">
    <div class="terminal-meta">
      <span>Attach count: {selectedSession?.attachCount ?? 0}</span>
      <span>Last activity: {formatRelativeTime(selectedSession?.lastActivityAt ?? selectedSession?.updatedAt)}</span>
    </div>
    <div class="terminal-widget" bind:this={terminalContainer}></div>
    <div class="terminal-input-row">
      <input
        bind:value={terminalInput}
        placeholder="Send input to selected terminal"
        onkeydown={(event) => event.key === "Enter" && sendTerminalInput()}
        disabled={!selectedSessionId}
      />
      <button onclick={sendTerminalInput} disabled={!selectedSessionId}>Send</button>
    </div>
  </section>

  {#if loading}
    <p class="loading">Refreshing session state…</p>
  {/if}
</main>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    height: 100%;
    background: #070e17;
    color: #dfe9f2;
    font-family: "Space Grotesk", "Avenir Next", "Segoe UI", sans-serif;
  }

  * {
    box-sizing: border-box;
  }

  .popout-shell {
    height: 100vh;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 10px;
    overflow: hidden;
    background-image: radial-gradient(circle at 10% 8%, rgba(47, 212, 195, 0.12), transparent 36%),
      radial-gradient(circle at 95% 2%, rgba(255, 184, 92, 0.12), transparent 34%);
  }

  .popout-header {
    border: 1px solid rgba(123, 161, 199, 0.35);
    border-radius: 12px;
    padding: 10px;
    background: rgba(8, 15, 24, 0.92);
    display: flex;
    justify-content: space-between;
    gap: 12px;
    align-items: center;
  }

  .popout-header h1 {
    margin: 0;
    font-size: 15px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .popout-header p {
    margin: 4px 0 0;
    font-size: 12px;
    color: rgba(223, 233, 242, 0.78);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  label {
    display: grid;
    gap: 4px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: rgba(223, 233, 242, 0.7);
  }

  select {
    min-width: 210px;
    border: 1px solid rgba(123, 161, 199, 0.45);
    border-radius: 10px;
    padding: 8px 10px;
    background: rgba(11, 19, 29, 0.95);
    color: inherit;
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

  .terminal-panel {
    flex: 1;
    min-height: 0;
    border: 1px solid rgba(123, 161, 199, 0.35);
    border-radius: 12px;
    padding: 10px;
    background: rgba(8, 15, 24, 0.92);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .terminal-meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 12px;
    color: rgba(223, 233, 242, 0.76);
  }

  .terminal-widget {
    flex: 1;
    min-height: 300px;
    overflow: hidden;
    background: #04080e;
    border: 1px solid rgba(123, 161, 199, 0.28);
    border-radius: 10px;
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

  .terminal-input-row input {
    border: 1px solid rgba(123, 161, 199, 0.45);
    border-radius: 10px;
    padding: 10px 12px;
    background: rgba(11, 19, 29, 0.95);
    color: inherit;
  }

  button {
    border: 1px solid rgba(47, 212, 195, 0.52);
    border-radius: 10px;
    padding: 8px 12px;
    font-size: 12px;
    font-weight: 700;
    background: linear-gradient(180deg, #3ae0cd 0%, #2fd4c3 100%);
    color: #042926;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .loading {
    margin: 0;
    font-size: 12px;
    color: rgba(223, 233, 242, 0.74);
  }

  @media (max-width: 840px) {
    .popout-header {
      flex-direction: column;
      align-items: stretch;
    }

    .header-actions {
      flex-direction: column;
      align-items: stretch;
    }

    select {
      min-width: 0;
      width: 100%;
    }

    .terminal-meta {
      flex-direction: column;
      align-items: flex-start;
      gap: 2px;
    }
  }
</style>
