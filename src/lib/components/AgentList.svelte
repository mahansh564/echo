<script lang="ts">
  type AgentRow = {
    id: number;
    name: string;
    state: string;
    provider: string;
    attentionState: string;
    unresolvedAlertCount: number;
    activeSessionStatus?: string | null;
    activeSessionNeedsInput?: boolean | null;
    activeSessionInputReason?: string | null;
    taskTitle?: string | null;
    taskId?: number | null;
    lastActivityAt?: string | null;
    lastSnippet?: string | null;
    updatedAt: string;
  };

  export let agents: AgentRow[] = [];

  export let selectedId: number | null = null;
  export let onSelect: (id: number) => void;
  export let onAttach: ((id: number) => void) | undefined;
  export let onReply: ((id: number) => void) | undefined;
  export let onAcknowledge: ((id: number) => void) | undefined;
  export let pendingAckAgentIds: number[] = [];

  const toTitleCase = (value: string) =>
    value
      .split(/[_\s-]+/)
      .filter(Boolean)
      .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
      .join(" ");

  const isInputNeeded = (agent: AgentRow) =>
    !!agent.activeSessionNeedsInput || agent.unresolvedAlertCount > 0;

  const selectByOffset = (offset: number) => {
    if (agents.length === 0) return;
    const index = Math.max(0, agents.findIndex((agent) => agent.id === selectedId));
    const next = Math.min(agents.length - 1, Math.max(0, index + offset));
    onSelect?.(agents[next].id);
  };

  function handleRailKeydown(event: KeyboardEvent) {
    if (agents.length === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      selectByOffset(1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      selectByOffset(-1);
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      onSelect?.(agents[0].id);
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      onSelect?.(agents[agents.length - 1].id);
    }
  }
</script>

<section class="panel">
  <header>
    <h2>Agent runtime</h2>
    <span class="count">{agents.length} total</span>
  </header>

  {#if agents.length === 0}
    <div class="empty">
      <p>No agents yet. Spawn an agent to begin.</p>
    </div>
  {:else}
    <div
      class="rail"
      tabindex="0"
      role="listbox"
      aria-label="Agent runtime rail"
      on:keydown={handleRailKeydown}
    >
      {#each agents as agent}
        <div
          role="option"
          tabindex="0"
          aria-selected={agent.id === selectedId}
          class:selected={agent.id === selectedId}
          class="card"
          on:click={() => onSelect?.(agent.id)}
          on:keydown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              onSelect?.(agent.id);
            }
          }}
        >
          <span class="card-head">
            <span class="identity">
              <strong>{agent.name}</strong>
              <small>{agent.provider}</small>
            </span>
            <span class="status {agent.activeSessionStatus ?? agent.state}">
              {toTitleCase(agent.activeSessionStatus ?? agent.state)}
            </span>
          </span>
          <span class="task">Task: {agent.taskTitle ?? "Unassigned"}</span>
          <span class="activity">Last activity: {agent.lastActivityAt ?? agent.updatedAt}</span>
          <span class="attention {isInputNeeded(agent) ? "needs_input" : agent.attentionState}">
            {isInputNeeded(agent) ? "Needs input" : toTitleCase(agent.attentionState)}
            {#if agent.unresolvedAlertCount > 0}
              <strong>({agent.unresolvedAlertCount})</strong>
            {/if}
          </span>
          <span class="snippet">{agent.lastSnippet ?? "Waiting for terminal output..."}</span>
          <span class="actions">
            <button class="ghost small" on:click|stopPropagation={() => onAttach?.(agent.id)}>
              Attach
            </button>
            <button class="ghost small" on:click|stopPropagation={() => onReply?.(agent.id)}>
              Reply
            </button>
            <button
              class="primary small"
              disabled={!pendingAckAgentIds.includes(agent.id)}
              on:click|stopPropagation={() => onAcknowledge?.(agent.id)}
            >
              Acknowledge
            </button>
          </span>
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .panel {
    background: rgba(16, 19, 24, 0.7);
    border-radius: 24px;
    padding: 24px;
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
    backdrop-filter: blur(16px);
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
  }

  h2 {
    margin: 0;
    font-size: 20px;
  }

  .count {
    font-size: 12px;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: rgba(244, 242, 238, 0.5);
  }

  .rail {
    display: grid;
    gap: 10px;
    max-height: 520px;
    overflow-y: auto;
    padding-right: 4px;
  }

  .rail:focus-visible {
    outline: 2px solid rgba(123, 223, 242, 0.6);
    outline-offset: 8px;
    border-radius: 18px;
  }

  .empty {
    padding: 24px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.02);
    color: rgba(244, 242, 238, 0.65);
    text-align: center;
  }

  .card {
    display: grid;
    gap: 10px;
    padding: 12px 14px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.02);
    border: 1px solid transparent;
    color: inherit;
    text-align: left;
  }

  .card {
    cursor: pointer;
    transition: border 0.2s ease, background 0.2s ease;
  }

  .card:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  .card.selected {
    border-color: rgba(255, 159, 67, 0.6);
    background: rgba(255, 159, 67, 0.08);
  }

  .card:focus-visible {
    outline: 2px solid rgba(123, 223, 242, 0.6);
    outline-offset: 2px;
  }

  .card-head {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    align-items: baseline;
  }

  .identity {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .identity strong {
    font-weight: 600;
  }

  .identity small {
    color: rgba(244, 242, 238, 0.55);
    font-size: 11px;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.12em;
  }

  .status {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.14em;
  }

  .status.active,
  .status.running {
    color: #ffd166;
  }

  .status.waking,
  .status.idle {
    color: #7bdff2;
  }

  .status.needs_input,
  .status.failed,
  .status.blocked {
    color: #ef476f;
  }

  .status.ended {
    color: rgba(244, 242, 238, 0.5);
  }

  .task,
  .activity,
  .attention,
  .snippet {
    color: rgba(244, 242, 238, 0.7);
    font-size: 13px;
  }

  .attention {
    text-transform: uppercase;
    letter-spacing: 0.1em;
    font-size: 11px;
  }

  .attention.ok {
    color: #7bdff2;
  }

  .attention.needs_input {
    color: #ffd166;
  }

  .attention.blocked {
    color: #ef476f;
  }

  .attention strong {
    margin-left: 4px;
    color: inherit;
  }

  .snippet {
    font-family: "JetBrains Mono", "Menlo", monospace;
    white-space: nowrap;
    text-overflow: ellipsis;
    overflow: hidden;
  }

  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
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

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
