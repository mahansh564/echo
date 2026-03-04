<script lang="ts">
  export let agent:
    | {
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
        lastActivityAt?: string | null;
        taskId?: number | null;
        lastSnippet?: string | null;
        updatedAt: string;
      }
    | undefined;
  export let task:
    | {
        id: number;
        title: string;
        state: string;
      }
    | undefined;
  export let onOpenSession: (() => void) | undefined;
  export let canOpenSession: boolean = true;
  export let linkedSession:
    | {
        id: number;
        status: string;
        launchCommand: string;
      }
    | undefined;
</script>

<section class="panel">
  {#if agent}
    <header>
      <div>
        <p class="eyebrow">Focused agent</p>
        <h2>{agent.name}</h2>
      </div>
      <span class="pill {agent.state}">{agent.state}</span>
    </header>

    <div class="meta">
      <div>
        <p class="label">Provider</p>
        <p class="value">{agent.provider}</p>
      </div>
      <div>
        <p class="label">Assigned task</p>
        <p class="value">{agent.taskTitle ?? task?.title ?? "No task assigned"}</p>
      </div>
      <div>
        <p class="label">Last activity</p>
        <p class="value">{agent.lastActivityAt ?? agent.updatedAt}</p>
      </div>
      <div>
        <p class="label">Linked session</p>
        <p class="value">
          {#if linkedSession}
            #{linkedSession.id} ({linkedSession.status}) via {linkedSession.launchCommand}
          {:else}
            No linked session
          {/if}
        </p>
      </div>
      <div>
        <p class="label">Attention state</p>
        <p class="value attention {agent.attentionState}">
          {agent.activeSessionNeedsInput || agent.unresolvedAlertCount > 0
            ? "needs_input"
            : agent.attentionState}
          {#if agent.unresolvedAlertCount > 0}
            ({agent.unresolvedAlertCount} open)
          {/if}
        </p>
      </div>
      <div>
        <p class="label">Input reason</p>
        <p class="value">
          {agent.activeSessionInputReason ??
            (agent.activeSessionNeedsInput ? "Session flagged for operator input" : "None")}
        </p>
      </div>
    </div>

    <div class="terminal">
      <p class="label">Terminal output</p>
      <pre>
{agent.lastSnippet ?? "Waiting for output..."}
      </pre>
      <div class="terminal-actions">
        <button class="ghost">Attach task</button>
        <button class="primary" disabled={!canOpenSession} on:click={() => onOpenSession?.()}>
          {linkedSession ? "Attach terminal" : "Open session"}
        </button>
      </div>
    </div>
  {:else}
    <p class="empty">Select an agent to see details.</p>
  {/if}
</section>

<style>
  .panel {
    background: rgba(12, 14, 18, 0.76);
    border-radius: 24px;
    padding: 24px;
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
    backdrop-filter: blur(18px);
    display: flex;
    flex-direction: column;
    gap: 18px;
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
  }

  h2 {
    margin: 6px 0 0;
    font-size: 24px;
  }

  .eyebrow {
    margin: 0;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.2em;
    color: rgba(244, 242, 238, 0.5);
  }

  .pill {
    padding: 6px 12px;
    border-radius: 999px;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    font-size: 11px;
  }

  .pill.running {
    background: rgba(255, 209, 102, 0.2);
    color: #ffd166;
  }

  .pill.idle {
    background: rgba(123, 223, 242, 0.2);
    color: #7bdff2;
  }

  .pill.blocked {
    background: rgba(239, 71, 111, 0.2);
    color: #ef476f;
  }

  .meta {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
  }

  .label {
    font-size: 12px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: rgba(244, 242, 238, 0.5);
    margin: 0 0 6px;
  }

  .value {
    margin: 0;
    font-size: 16px;
  }

  .value.attention.needs_input,
  .value.attention.blocked {
    color: #ffd166;
  }

  .terminal {
    background: #0b0d10;
    border-radius: 20px;
    padding: 18px;
    border: 1px solid rgba(255, 255, 255, 0.06);
  }

  pre {
    font-family: "JetBrains Mono", "Menlo", monospace;
    margin: 0 0 12px;
    color: rgba(244, 242, 238, 0.8);
    white-space: pre-wrap;
  }

  .terminal-actions {
    display: flex;
    gap: 10px;
  }

  button {
    border: none;
    padding: 8px 14px;
    border-radius: 999px;
    font-weight: 600;
    cursor: pointer;
  }

  button:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .ghost {
    background: rgba(255, 255, 255, 0.08);
    color: #f4f2ee;
  }

  .primary {
    background: linear-gradient(120deg, #63b3ff, #9b5cff);
    color: #09080b;
  }

  .empty {
    margin: 0;
    color: rgba(244, 242, 238, 0.6);
  }

  @media (max-width: 980px) {
    .meta {
      grid-template-columns: 1fr;
    }
  }
</style>
