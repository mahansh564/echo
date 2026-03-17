<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
  import { onMount } from "svelte";

  type SessionStatus = "waking" | "active" | "stalled" | "needs_input" | "ended" | "failed";

  type SessionRuntime = {
    id: number;
    status: SessionStatus;
    provider: string;
    agentId?: number | null;
    lastActivityAt?: string | null;
    needsInput?: boolean;
    inputReason?: string | null;
    updatedAt: string;
  };

  type AgentRow = {
    id: number;
    name: string;
    activeSessionId?: number | null;
  };

  type SessionView = {
    id: number;
    status: SessionStatus;
    provider: string;
    needsInput: boolean;
    inputReason?: string | null;
    lastActivityAt?: string | null;
    updatedAt: string;
    agentName: string;
  };

  type UnlistenFn = () => void;

  const ACTIVE_STATUSES = new Set<SessionStatus>(["waking", "active", "stalled", "needs_input"]);
  const TERMINAL_POPOUT_WINDOW_LABEL = "terminal-popout";
  const refreshEveryMs = 4000;

  let loading = true;
  let switchingToFull = false;
  let rows: SessionView[] = [];
  let refreshTimer = 0;
  let periodicTimer = 0;
  let unlisteners: UnlistenFn[] = [];

  const toTitleCase = (value: string) =>
    value
      .split(/[\s_-]+/)
      .filter(Boolean)
      .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
      .join(" ");

  function formatRelativeTime(value: string | null | undefined) {
    if (!value) return "just now";
    const normalized = value.includes("T") ? value : value.replace(" ", "T") + "Z";
    const date = new Date(normalized);
    if (Number.isNaN(date.getTime())) return value;
    const diffSeconds = Math.max(0, Math.floor((Date.now() - date.getTime()) / 1000));
    if (diffSeconds < 10) return "just now";
    if (diffSeconds < 60) return `${diffSeconds}s ago`;
    const minutes = Math.floor(diffSeconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    return `${Math.floor(hours / 24)}d ago`;
  }

  const rowSortKey = (row: SessionView) => {
    const last = row.lastActivityAt ?? row.updatedAt;
    const stamp = new Date(last.includes("T") ? last : `${last.replace(" ", "T")}Z`).getTime();
    return Number.isFinite(stamp) ? stamp : 0;
  };

  async function loadRows() {
    try {
      const [sessions, agents] = await Promise.all([
        invoke("list_managed_sessions_cmd", { status: null, limit: 200 }) as Promise<SessionRuntime[]>,
        invoke("list_agent_rows_cmd", { limit: 200 }) as Promise<AgentRow[]>
      ]);

      const byAgentId = new Map<number, AgentRow>();
      const byActiveSessionId = new Map<number, AgentRow>();
      for (const agent of agents) {
        byAgentId.set(agent.id, agent);
        if (agent.activeSessionId) byActiveSessionId.set(agent.activeSessionId, agent);
      }

      rows = sessions
        .filter((session) => ACTIVE_STATUSES.has(session.status))
        .map((session) => {
          const linked =
            (session.agentId ? byAgentId.get(session.agentId) : undefined) ??
            byActiveSessionId.get(session.id);
          return {
            id: session.id,
            status: session.status,
            provider: session.provider,
            needsInput: !!session.needsInput || session.status === "needs_input",
            inputReason: session.inputReason ?? null,
            lastActivityAt: session.lastActivityAt ?? null,
            updatedAt: session.updatedAt,
            agentName: linked?.name ?? "Unassigned"
          } satisfies SessionView;
        })
        .sort((a, b) => {
          if (a.needsInput !== b.needsInput) return a.needsInput ? -1 : 1;
          return rowSortKey(b) - rowSortKey(a);
        });
    } catch (error) {
      console.error("Failed to load zen panel sessions", error);
    } finally {
      loading = false;
    }
  }

  function scheduleRefresh(delayMs = 300) {
    if (typeof window === "undefined") return;
    if (refreshTimer) window.clearTimeout(refreshTimer);
    refreshTimer = window.setTimeout(() => {
      refreshTimer = 0;
      void loadRows();
    }, delayMs);
  }

  async function openTerminalPopout(sessionId: number) {
    const popoutUrl = `/terminal-popout?sessionId=${sessionId}`;

    try {
      const existing = await WebviewWindow.getByLabel(TERMINAL_POPOUT_WINDOW_LABEL);
      if (existing) {
        await existing.emit("terminal-popout-focus-session", { sessionId });
        await existing.show();
        await existing.setFocus();
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

  async function switchToFullMode() {
    if (switchingToFull) return;
    switchingToFull = true;
    try {
      await invoke("set_app_mode_cmd", { mode: "full" });
    } catch (error) {
      console.error("Failed to switch app mode to full", error);
      switchingToFull = false;
    }
  }

  onMount(() => {
    void loadRows();

    const register = async () => {
      unlisteners.push(
        await listen("agent_runtime_updated", () => scheduleRefresh()),
        await listen("agent_attention_updated", () => scheduleRefresh()),
        await listen("session_alert_created", () => scheduleRefresh()),
        await listen("session_alert_resolved", () => scheduleRefresh()),
        await listen("session_alert_snoozed", () => scheduleRefresh()),
        await listen("session_alert_escalated", () => scheduleRefresh()),
        await listen("terminal_chunk", () => scheduleRefresh(600))
      );
    };

    void register();

    periodicTimer = window.setInterval(() => {
      void loadRows();
    }, refreshEveryMs);

    const resume = () => scheduleRefresh(0);
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", resume);

    return () => {
      if (refreshTimer) window.clearTimeout(refreshTimer);
      if (periodicTimer) window.clearInterval(periodicTimer);
      for (const unlisten of unlisteners) unlisten();
      unlisteners = [];
      window.removeEventListener("focus", resume);
      document.removeEventListener("visibilitychange", resume);
    };
  });
</script>

<main>
  <header>
    <div class="heading">
      <h1>Zen Sessions</h1>
      <p>{rows.length} active</p>
    </div>
    <button class="mode-toggle" on:click={switchToFullMode} disabled={switchingToFull}>
      <span class="mode-label active">Zen</span>
      <span class="mode-label">{switchingToFull ? "..." : "Full"}</span>
    </button>
  </header>

  {#if loading}
    <p class="hint">Loading active sessions...</p>
  {:else if rows.length === 0}
    <p class="hint">No active sessions right now.</p>
  {:else}
    <section class="list" aria-label="Active sessions">
      {#each rows as row}
        <button class="row" on:click={() => openTerminalPopout(row.id)}>
          <span class="top">
            <strong>{row.agentName}</strong>
            <span class="status {row.status}">{toTitleCase(row.status)}</span>
          </span>
          <span class="meta">Session #{row.id} · {row.provider}</span>
          <span class="meta">
            Last activity {formatRelativeTime(row.lastActivityAt ?? row.updatedAt)}
            {#if row.needsInput}
              · Needs input
            {/if}
          </span>
        </button>
      {/each}
    </section>
  {/if}
</main>

<style>
  :global(html, body) {
    margin: 0;
    background: #121417;
    color: #f3f4f6;
    font-family: "SF Pro Text", "Inter", -apple-system, sans-serif;
  }

  main {
    padding: 14px;
    display: grid;
    gap: 10px;
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
  }

  .heading {
    display: grid;
    gap: 2px;
  }

  h1 {
    margin: 0;
    font-size: 14px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  header p {
    margin: 0;
    font-size: 11px;
    color: #9aa3b2;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .mode-toggle {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    border: 1px solid #2a303b;
    border-radius: 999px;
    background: #171b21;
    color: #a7b0bf;
    padding: 2px;
    font-size: 11px;
    letter-spacing: 0.03em;
    cursor: pointer;
    min-width: 88px;
  }

  .mode-toggle:hover:enabled {
    border-color: #3a4352;
    background: #1b2028;
  }

  .mode-toggle:disabled {
    opacity: 0.72;
    cursor: progress;
  }

  .mode-label {
    min-width: 36px;
    text-align: center;
    padding: 3px 6px;
    border-radius: 999px;
  }

  .mode-label.active {
    background: #222934;
    color: #e5e7eb;
  }

  .hint {
    margin: 0;
    font-size: 13px;
    color: #98a2b3;
  }

  .list {
    display: grid;
    gap: 8px;
    max-height: 500px;
    overflow: auto;
  }

  .row {
    width: 100%;
    text-align: left;
    border: 1px solid #252a33;
    border-radius: 10px;
    background: #181c22;
    color: inherit;
    padding: 10px 11px;
    display: grid;
    gap: 3px;
    cursor: pointer;
  }

  .row:hover {
    border-color: #3a4250;
    background: #1d222a;
  }

  .top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .top strong {
    font-size: 13px;
    font-weight: 600;
  }

  .status {
    font-size: 10px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: #a7b0bf;
  }

  .status.active {
    color: #7dd3fc;
  }

  .status.needs_input,
  .status.stalled {
    color: #fbbf24;
  }

  .status.failed {
    color: #f87171;
  }

  .meta {
    font-size: 11px;
    color: #98a2b3;
  }
</style>
