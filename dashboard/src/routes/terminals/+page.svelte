<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { sessions, loadSessions } from '$stores/sessions';
  import { layouts, ensureLayouts, resetLayout } from '$stores/canvas';
  import { fullscreen, toggleFullscreen } from '$stores/fullscreen';
  import TerminalPanel from '$components/TerminalPanel.svelte';
  import EmptyState from '$components/EmptyState.svelte';

  let showRunningOnly = $state(true);
  let maximizedId = $state<string | null>(null);

  // Visible sessions: optionally filter to running, since dead sessions have
  // no live pane to interact with.
  let visible = $derived.by(() => {
    const items = $sessions.items;
    return showRunningOnly ? items.filter((s) => s.status === 'running') : items;
  });

  // Whenever the visible set changes, make sure each has a layout entry.
  $effect(() => {
    ensureLayouts(visible.map((s) => s.id));
  });

  // Canvas dims need to grow to fit the furthest panel so the user can scroll.
  let canvasH = $derived.by(() => {
    let max = 600;
    for (const s of visible) {
      const l = $layouts[s.id];
      if (!l) continue;
      max = Math.max(max, l.y + l.h + 32);
    }
    return max;
  });
  let canvasW = $derived.by(() => {
    let max = 1200;
    for (const s of visible) {
      const l = $layouts[s.id];
      if (!l) continue;
      max = Math.max(max, l.x + l.w + 32);
    }
    return max;
  });

  function refresh() {
    loadSessions();
  }

  function onMaximize(id: string) {
    maximizedId = maximizedId === id ? null : id;
  }
  function onOpen(id: string) {
    goto(`/sessions/${id}`);
  }
  function onResetLayout() {
    if (!confirm('Reset panel positions and sizes for all visible terminals?')) return;
    resetLayout(visible.map((s) => s.id));
  }

  onMount(() => {
    refresh();
    const tick = setInterval(refresh, 5000);
    return () => clearInterval(tick);
  });
</script>

<section class="head">
  <div>
    <h2>Terminals</h2>
    <p class="muted">Drag headers to move, drag edges/corner to resize. Double-click a header to maximize.</p>
  </div>
  <div class="actions">
    <label class="toggle">
      <input type="checkbox" bind:checked={showRunningOnly} />
      <span>running only</span>
    </label>
    <button class="ghost" type="button" onclick={onResetLayout} title="Tile panels in a grid">
      reset layout
    </button>
    <button class="ghost" type="button" onclick={toggleFullscreen} title="Fullscreen (Shift+F)">
      {$fullscreen ? '⤢ exit fullscreen' : '⤢ fullscreen'}
    </button>
  </div>
</section>

{#if visible.length === 0}
  <EmptyState
    title={showRunningOnly ? 'No running sessions' : 'No sessions'}
    body={showRunningOnly
      ? 'Start a session, or untick "running only" to view stopped panes.'
      : 'Create a session from the Agents page first.'}
  />
{:else}
  <div
    class="canvas"
    class:fullscreen={$fullscreen}
    style:--canvas-w="{canvasW}px"
    style:--canvas-h="{canvasH}px"
  >
    {#each visible as session (session.id)}
      {#if $layouts[session.id]}
        <TerminalPanel
          {session}
          layout={$layouts[session.id]}
          maximized={maximizedId === session.id}
          {onMaximize}
          {onOpen}
        />
      {/if}
    {/each}
  </div>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    margin-bottom: 0.9rem;
    gap: 1rem;
    flex-wrap: wrap;
  }
  h2 {
    font-family: var(--font-display);
    font-weight: 600;
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  .muted { color: var(--muted); margin: 0; font-size: 0.85rem; }
  .actions { display: flex; gap: 0.5rem; align-items: center; }
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-family: var(--font-mono);
    font-size: 0.78rem;
    color: var(--text-2);
    cursor: pointer;
  }
  .toggle input { accent-color: var(--accent); }
  .ghost {
    background: var(--surface);
    color: var(--text-2);
    border: 1px solid var(--border);
    padding: 0.4rem 0.75rem;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.78rem;
    cursor: pointer;
  }
  .ghost:hover {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }

  .canvas {
    position: relative;
    width: 100%;
    min-width: var(--canvas-w);
    height: var(--canvas-h);
    background:
      repeating-linear-gradient(
        0deg,
        transparent 0,
        transparent 23px,
        color-mix(in srgb, var(--border) 35%, transparent) 24px
      ),
      repeating-linear-gradient(
        90deg,
        transparent 0,
        transparent 23px,
        color-mix(in srgb, var(--border) 35%, transparent) 24px
      ),
      var(--bg);
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    overflow: auto;
  }
  .canvas.fullscreen {
    border-radius: 0;
    border: 0;
    height: calc(100vh - 0px);
    min-height: 100vh;
  }
</style>
