<script lang="ts">
  export let agents: {
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
  }[] = [];

  export let selectedId: number | null = null;
  export let onSelect: (id: number) => void;
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
    <div class="table">
      <div class="row header">
        <span>Agent</span>
        <span>Status</span>
        <span>Task</span>
        <span>Activity</span>
        <span>Attention</span>
        <span>Snippet</span>
      </div>
      {#each agents as agent}
        <button
          type="button"
          class:selected={agent.id === selectedId}
          class="row"
          on:click={() => onSelect?.(agent.id)}
        >
          <span class="name">
            {agent.name}
            <small>{agent.provider}</small>
          </span>
          <span class="state {agent.state}">{agent.state}</span>
          <span class="task">{agent.taskTitle ?? "—"}</span>
          <span class="updated">{agent.lastActivityAt ?? agent.updatedAt}</span>
          <span class="attention {agent.attentionState}">
            {agent.activeSessionNeedsInput || agent.unresolvedAlertCount > 0 ? "needs input" : agent.attentionState}
            {#if agent.unresolvedAlertCount > 0}
              <strong>({agent.unresolvedAlertCount})</strong>
            {/if}
          </span>
          <span class="snippet">{agent.lastSnippet ?? "—"}</span>
        </button>
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

  .table {
    display: grid;
    gap: 6px;
    max-height: 520px;
    overflow-y: auto;
    padding-right: 4px;
  }

  .empty {
    padding: 24px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.02);
    color: rgba(244, 242, 238, 0.65);
    text-align: center;
  }

  .row {
    display: grid;
    grid-template-columns: 1.2fr 0.75fr 1.2fr 0.8fr 1fr 1.2fr;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 16px;
    background: rgba(255, 255, 255, 0.02);
    border: 1px solid transparent;
    color: inherit;
    text-align: left;
  }

  .row.header {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.16em;
    color: rgba(244, 242, 238, 0.4);
    background: transparent;
  }

  button.row {
    cursor: pointer;
    transition: border 0.2s ease, background 0.2s ease;
  }

  button.row:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  button.row.selected {
    border-color: rgba(255, 159, 67, 0.6);
    background: rgba(255, 159, 67, 0.08);
  }

  .name {
    display: flex;
    flex-direction: column;
    font-weight: 600;
  }

  .name small {
    color: rgba(244, 242, 238, 0.55);
    font-size: 11px;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    margin-top: 2px;
  }

  .state {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.14em;
  }

  .state.running {
    color: #ffd166;
  }

  .state.idle {
    color: #7bdff2;
  }

  .state.blocked {
    color: #ef476f;
  }

  .task,
  .updated,
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
  }
</style>
