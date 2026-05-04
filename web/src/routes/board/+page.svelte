<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/api';
  import { board, loadBoard, moveLocal } from '$stores/board';
  import BoardCard from '$components/BoardCard.svelte';

  let pollHandle: ReturnType<typeof setInterval> | null = null;
  let dragOverColumn = $state<string | null>(null);
  let creating = $state(false);
  let newTitle = $state('');
  let newColumn = $state('todo');
  let actionError = $state<string | null>(null);

  onMount(() => {
    loadBoard();
    pollHandle = setInterval(loadBoard, 5000);
  });
  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
  });

  function showError(msg: string) {
    actionError = msg;
    setTimeout(() => {
      if (actionError === msg) actionError = null;
    }, 6000);
  }

  async function createItem(e: Event) {
    e.preventDefault();
    if (!newTitle.trim()) return;
    creating = true;
    try {
      await api.createBoardItem({ title: newTitle.trim(), status: newColumn });
      newTitle = '';
      await loadBoard();
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    } finally {
      creating = false;
    }
  }

  function ondragover(e: DragEvent, column: string) {
    if (!e.dataTransfer) return;
    if (e.dataTransfer.types.includes('application/x-agentum-board-item')) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      dragOverColumn = column;
    }
  }

  async function ondrop(e: DragEvent, column: string) {
    e.preventDefault();
    dragOverColumn = null;
    const raw = e.dataTransfer?.getData('application/x-agentum-board-item');
    if (!raw) return;
    const id = parseInt(raw, 10);
    if (Number.isNaN(id)) return;

    moveLocal(id, column);
    try {
      await api.patchBoardItem(id, { status: column });
      await loadBoard();
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
      await loadBoard();
    }
  }
</script>

<section class="head">
  <div>
    <h2>Board</h2>
    <p class="muted">Atomic-claim kanban for cross-agent task handoff.</p>
  </div>
  <form class="new" onsubmit={createItem}>
    <input
      type="text"
      bind:value={newTitle}
      placeholder="new task title…"
      autocomplete="off"
      maxlength="120"
    />
    <select bind:value={newColumn}>
      <option value="todo">todo</option>
      <option value="doing">doing</option>
      <option value="done">done</option>
    </select>
    <button type="submit" class="primary" disabled={creating || !newTitle.trim()}>
      + add
    </button>
  </form>
</section>

{#if actionError}
  <div class="action-error">{actionError}</div>
{/if}

{#if $board.error}
  <div class="error">Failed to load: <code>{$board.error}</code></div>
{:else if $board.loading && !$board.data}
  <div class="muted">loading…</div>
{:else if $board.data}
  <div class="kanban">
    {#each $board.data.column_order as col}
      <div
        class="column"
        class:over={dragOverColumn === col}
        ondragover={(e) => ondragover(e, col)}
        ondragleave={() => (dragOverColumn = null)}
        ondrop={(e) => ondrop(e, col)}
        role="list"
      >
        <header>
          <span class="col-name">{col}</span>
          <span class="col-count">{$board.data.columns[col]?.length ?? 0}</span>
        </header>
        <div class="cards">
          {#each $board.data.columns[col] ?? [] as item (item.id)}
            <BoardCard {item} onerror={showError} />
          {/each}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 1rem;
    margin-bottom: 1rem;
    flex-wrap: wrap;
  }
  h2 {
    margin: 0 0 0.25rem;
    font-family: var(--font-display);
    font-size: 1.4rem;
    font-weight: 600;
  }
  .muted { color: var(--muted); margin: 0; }
  .new {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .new input, .new select {
    padding: 0.45rem 0.7rem;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .new input { min-width: 14rem; }
  .new input:focus, .new select:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .primary {
    background: var(--accent);
    color: var(--bg);
    padding: 0.45rem 0.9rem;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .primary:disabled { opacity: 0.5; cursor: not-allowed; }

  .action-error {
    margin-bottom: 0.7rem;
    padding: 0.5rem 0.8rem;
    border: 1px solid var(--danger);
    border-radius: 6px;
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }

  .kanban {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 0.85rem;
  }
  .column {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface-2);
    min-height: 240px;
    transition: border-color 120ms ease, background 120ms ease;
  }
  .column.over {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 6%, var(--surface-2));
  }
  .column header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    padding: 0.1rem 0.3rem 0.4rem;
    border-bottom: 1px solid var(--border);
  }
  .col-name { letter-spacing: 0.05em; text-transform: uppercase; }
  .col-count {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 0.5em;
    font-size: 0.72rem;
    color: var(--muted);
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-height: 60px;
  }
  .error {
    padding: 0.7rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-family: var(--font-mono);
  }
  code { font-family: var(--font-mono); }
</style>
