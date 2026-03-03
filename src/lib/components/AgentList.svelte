<script lang="ts">
  export let agents: {
    id: number;
    name: string;
    state: string;
    taskId?: number | null;
    lastSnippet?: string | null;
    updatedAt: string;
  }[] = [];

  export let tasks: { id: number; title: string; state: string; updatedAt?: string }[] = [];
  export let selectedId: number | null = null;
  export let onSelect: (id: number) => void;

  const lookupTask = (taskId?: number | null) =>
    tasks.find((task) => task.id === taskId);
</script>

<section class="panel">
  <header>
    <h2>Active agents</h2>
    <span class="count">{agents.length} online</span>
  </header>

  {#if agents.length === 0}
    <div class="empty">
      <p>No agents yet. Spawn an agent to begin.</p>
    </div>
  {:else}
    <div class="table">
      <div class="row header">
        <span>Name</span>
        <span>Status</span>
        <span>Task</span>
        <span>Last update</span>
        <span>Snippet</span>
      </div>
      {#each agents as agent}
        <button
          type="button"
          class:selected={agent.id === selectedId}
          class="row"
          on:click={() => onSelect?.(agent.id)}
        >
          <span class="name">{agent.name}</span>
          <span class="state {agent.state}">{agent.state}</span>
          <span class="task">{lookupTask(agent.taskId)?.title ?? "—"}</span>
          <span class="updated">{agent.updatedAt}</span>
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
    grid-template-columns: 1.2fr 0.7fr 1.4fr 0.9fr 1.4fr;
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
    font-weight: 600;
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
  .snippet {
    color: rgba(244, 242, 238, 0.7);
    font-size: 13px;
  }

  .snippet {
    font-family: "JetBrains Mono", "Menlo", monospace;
  }
</style>
