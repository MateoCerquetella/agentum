<script lang="ts">
  import type { BoardItem } from '$lib/api';
  import { api } from '$lib/api';
  import { actorId } from '$stores/actor';
  import { loadBoard } from '$stores/board';

  interface Props {
    item: BoardItem;
    onerror: (msg: string) => void;
  }
  let { item, onerror }: Props = $props();

  let claiming = $state(false);

  async function claim() {
    claiming = true;
    try {
      await api.claimBoardItem(item.id, actorId());
      await loadBoard();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      onerror(msg);
    } finally {
      claiming = false;
    }
  }

  async function release() {
    // Atomic claim doesn't have an "unclaim" endpoint in phase 7; PATCH the
    // claimed_by indirectly by deleting + re-creating? No — release belongs
    // to a future phase. For now, hide the unclaim affordance.
  }

  async function remove() {
    if (!confirm(`Delete ${item.key}?`)) return;
    try {
      await api.deleteBoardItem(item.id);
      await loadBoard();
    } catch (e) {
      onerror(e instanceof Error ? e.message : String(e));
    }
  }

  function dragstart(e: DragEvent) {
    if (!e.dataTransfer) return;
    e.dataTransfer.setData('application/x-agentum-board-item', String(item.id));
    e.dataTransfer.effectAllowed = 'move';
  }

  const mine = $derived(item.claimed_by === actorId());
</script>

<article
  class="card"
  class:claimed={!!item.claimed_by}
  class:mine
  draggable="true"
  ondragstart={dragstart}
>
  <header>
    <span class="key">{item.key}</span>
    <button class="x" type="button" onclick={remove} title="delete">×</button>
  </header>
  <div class="title">{item.title}</div>
  {#if item.body}<div class="body">{item.body}</div>{/if}
  <footer>
    {#if item.claimed_by}
      <span class="claimed-by" title="claimed by">
        {mine ? 'you' : item.claimed_by}
      </span>
    {:else}
      <button type="button" class="claim" onclick={claim} disabled={claiming}>
        {claiming ? 'claiming…' : 'claim'}
      </button>
    {/if}
  </footer>
</article>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.7rem 0.8rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    cursor: grab;
    user-select: none;
    transition: border-color 120ms ease, transform 120ms ease;
  }
  .card:active { cursor: grabbing; }
  .card:hover {
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
  }
  .card.claimed { opacity: 0.92; }
  .card.mine { border-left: 3px solid var(--accent); }

  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--muted);
  }
  .key { letter-spacing: 0.04em; }
  .x {
    color: var(--muted);
    font-size: 1rem;
    line-height: 1;
    padding: 0 0.3rem;
    border-radius: 4px;
  }
  .x:hover { color: var(--danger); background: var(--surface-2); }

  .title {
    font-family: var(--font-display);
    font-size: 0.95rem;
    font-weight: 600;
    color: var(--text);
    line-height: 1.3;
  }
  .body {
    font-size: 0.83rem;
    color: var(--text-2);
    white-space: pre-wrap;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    margin-top: 0.2rem;
    min-height: 1.4rem;
  }
  .claimed-by {
    font-family: var(--font-mono);
    font-size: 0.72rem;
    padding: 0.1em 0.45em;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
  }
  .claim {
    font-family: var(--font-mono);
    font-size: 0.78rem;
    padding: 0.25rem 0.6rem;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text);
  }
  .claim:hover:not(:disabled) {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
    color: var(--accent);
  }
  .claim:disabled { opacity: 0.6; cursor: not-allowed; }
</style>
