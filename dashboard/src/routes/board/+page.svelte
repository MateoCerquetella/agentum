<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { board, loadBoard, moveLocal } from '$stores/board';
  import { sessions, loadSessions } from '$stores/sessions';
  import { watchdog, loadWatchdog } from '$stores/watchdog';
  import { api, type BoardItem } from '$lib/api';
  import { deriveState, fmtTokens, fmtCost } from '$lib/dashboard';
  import Ticket from '$components/dashboard/Ticket.svelte';
  import Watchdog from '$components/dashboard/Watchdog.svelte';

  let pollId: ReturnType<typeof setInterval> | null = null;
  let draggingId = $state<number | null>(null);
  let dropTargetCol = $state<string | null>(null);

  function refresh() {
    loadBoard();
    loadSessions();
    loadWatchdog(30);
  }

  onMount(() => {
    refresh();
    pollId = setInterval(refresh, 5000);
  });
  onDestroy(() => { if (pollId) clearInterval(pollId); });

  const cols = $derived.by(() => {
    const data = $board.data;
    if (!data) return [] as Array<{ key: string; label: string; items: BoardItem[]; tone: 'default' | 'live' | 'warn' | 'done' }>;
    return data.column_order.map(key => {
      const items = data.columns[key] ?? [];
      const k = key.toLowerCase();
      let tone: 'default' | 'live' | 'warn' | 'done' = 'default';
      if (/claimed|progress/.test(k)) tone = 'live';
      else if (/review|pr/.test(k)) tone = 'warn';
      else if (/done|shipped|merged/.test(k)) tone = 'done';
      return { key, label: prettify(key), items, tone };
    });
  });

  function prettify(key: string): string {
    return key.replace(/[_-]/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
  }

  /* -- drag and drop ------------------------------------------------- */
  function onTicketDragStart(item: BoardItem) {
    return (e: DragEvent) => {
      draggingId = item.id;
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', String(item.id));
      }
    };
  }
  function onTicketDragEnd() {
    draggingId = null;
    dropTargetCol = null;
  }
  function onColDragOver(colKey: string) {
    return (e: DragEvent) => {
      if (draggingId == null) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
      dropTargetCol = colKey;
    };
  }
  function onColDragLeave(colKey: string) {
    return () => { if (dropTargetCol === colKey) dropTargetCol = null; };
  }
  function onColDrop(colKey: string) {
    return async (e: DragEvent) => {
      e.preventDefault();
      const idStr = e.dataTransfer?.getData('text/plain') ?? '';
      const id = parseInt(idStr, 10);
      const wasDraggingId = draggingId;
      draggingId = null;
      dropTargetCol = null;
      if (!Number.isFinite(id)) return;
      // Optimistic local update first; reconcile on next loadBoard().
      moveLocal(id, colKey);
      try {
        await api.patchBoardItem(id, { status: colKey });
      } catch (e) {
        console.error('move failed', e);
        await loadBoard();
      }
    };
  }

  function openTicket(_tk: BoardItem) {
    // No detail screen yet — clicks are no-ops. Drag remains the
    // primary affordance.
  }

  /* -- right-rail metrics ------------------------------------------- */
  const live = $derived($sessions.items.filter(s => deriveState(s) === 'live'));
  const claimedCount = $derived.by(() => {
    const data = $board.data;
    if (!data) return 0;
    return Object.entries(data.columns)
      .filter(([k]) => /claimed|progress|review/i.test(k))
      .reduce((a, [, v]) => a + v.length, 0);
  });
  const reviewCount = $derived.by(() => {
    const data = $board.data;
    if (!data) return 0;
    return Object.entries(data.columns)
      .filter(([k]) => /review|pr/i.test(k))
      .reduce((a, [, v]) => a + v.length, 0);
  });
  const doneCount = $derived.by(() => {
    const data = $board.data;
    if (!data) return 0;
    return Object.entries(data.columns)
      .filter(([k]) => /done|shipped|merged/i.test(k))
      .reduce((a, [, v]) => a + v.length, 0);
  });
  const totalCount = $derived(cols.reduce((a, c) => a + c.items.length, 0));
  const tokens24 = $derived($sessions.items.reduce((a, s) => a + (s.tokens ?? 0), 0));
  const spend24  = $derived($sessions.items.reduce((a, s) => a + (s.cost ?? 0), 0));
</script>

<div class="page">
  <!-- Toolbar -->
  <div class="toolbar">
    <div class="tabs">
      <a class="tab" href="/sessions">Sessions <span class="badge">{$sessions.items.length}</span></a>
      <button type="button" class="tab active">Board <span class="badge">{totalCount}</span></button>
    </div>
    <span class="spacer"></span>
    <span class="pill"><span style="color: var(--fg-3);">group:</span>&nbsp;status</span>
    <span class="pill"><span style="color: var(--fg-3);">assignee:</span>&nbsp;all agents</span>
    <button type="button" class="tb-btn">Filter</button>
    <button type="button" class="tb-btn primary">+ Ticket</button>
  </div>

  <!-- Session strip -->
  <div class="strip">
    {#each $sessions.items.slice(0, 12) as s (s.id)}
      <a class="chip" href={`/sessions/${s.id}`}>
        <span
          class="dot"
          style:background={
            deriveState(s) === 'live' ? 'var(--green)' :
            deriveState(s) === 'compact' ? 'var(--cta)' :
            deriveState(s) === 'crash' ? 'var(--crash)' : 'var(--fg-3)'
          }
        ></span>
        <span class="nm">{s.name}</span>
        {#if s.tokens != null}
          <span class="meta">· {fmtTokens(s.tokens)}</span>
        {/if}
      </a>
    {/each}
    {#if $sessions.items.length === 0}
      <span class="empty">No sessions running.</span>
    {/if}
  </div>

  <!-- Board + rail -->
  <div class="row">
    <div class="board-wrap">
      {#if $board.loading && !$board.data}
        <div class="empty mono">Loading board…</div>
      {:else if $board.error}
        <div class="empty mono err">Failed to load board: {$board.error}</div>
      {:else if cols.length === 0}
        <div class="empty mono">Board has no columns yet.</div>
      {:else}
        <div class="board" style:grid-template-columns={`repeat(${Math.min(cols.length, 4)}, minmax(0, 1fr))`}>
          {#each cols as col (col.key)}
            <div
              class="col"
              class:drop-target={dropTargetCol === col.key}
              ondragover={onColDragOver(col.key)}
              ondragleave={onColDragLeave(col.key)}
              ondrop={onColDrop(col.key)}
              role="region"
              aria-label={col.label}
            >
              <div class="col-h">
                {#if col.tone === 'live'}<span style="color: var(--green);">●</span>
                {:else if col.tone === 'warn'}<span style="color: var(--amber);">●</span>
                {:else if col.tone === 'done'}<span style="color: var(--fg-3);">●</span>{/if}
                <span>{col.label}</span>
                <span class="count">{col.items.length}</span>
                <button type="button" class="add" aria-label={`Add to ${col.label}`}>+</button>
              </div>
              <div class="col-b">
                {#each col.items as tk (tk.id)}
                  <Ticket
                    {tk}
                    dragging={draggingId === tk.id}
                    onDragStart={onTicketDragStart(tk)}
                    onDragEnd={onTicketDragEnd}
                    onClick={() => openTicket(tk)}
                  />
                {/each}
                {#if col.items.length === 0}
                  <div class="col-empty mono">empty</div>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <aside class="rail" style="width: 300px;">
      <div class="rh">
        <span>watchdog</span>
        <span class="spacer"></span>
        <span class="pill live" style="font-size: 10px;">live</span>
      </div>
      <div class="rb">
        <div class="group">
          <div class="gh">
            <span>Last 30 min</span>
            <span style="color: var(--fg-2);">{$watchdog.items.length} event{$watchdog.items.length === 1 ? '' : 's'}</span>
          </div>
          <Watchdog feed={$watchdog.items} limit={20} />
        </div>
        <div class="group">
          <div class="gh">
            <span>Burn-down</span>
            <span style={doneCount >= claimedCount ? 'color: var(--green);' : 'color: var(--amber);'}>
              {doneCount >= claimedCount ? 'on track' : 'behind'}
            </span>
          </div>
          <div class="kv">
            <span class="k">claimed</span>   <span class="v">{claimedCount} / {totalCount}</span>
            <span class="k">in review</span> <span class="v">{reviewCount}</span>
            <span class="k">done today</span><span class="v"><span class="acc">{doneCount}</span></span>
            <span class="k">live</span>      <span class="v">{live.length}</span>
            <span class="k">tokens 24h</span><span class="v">{fmtTokens(tokens24)}</span>
            <span class="k">spend 24h</span> <span class="v"><span class="acc">{fmtCost(spend24)}</span></span>
          </div>
        </div>
      </div>
    </aside>
  </div>
</div>

<style>
  .page {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }
  .row {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .board-wrap {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  /* Session strip — compact horizontal scroll above the board */
  .strip {
    display: flex;
    gap: 8px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    background: #0d0d0d;
    overflow-x: auto;
    flex-shrink: 0;
  }
  .strip .chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px;
    border-radius: var(--radius-pill);
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    font-family: var(--mono);
    font-size: 11px;
    color: var(--fg-2);
    white-space: nowrap;
    text-decoration: none;
    transition: border-color var(--t-hover);
  }
  .strip .chip:hover { border-color: var(--fg-3); }
  .strip .chip .dot {
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill);
  }
  .strip .chip .nm { color: var(--fg); }
  .strip .chip .meta { color: var(--fg-3); }
  .strip .empty {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--fg-3);
    padding: 5px 0;
  }

  .col-empty {
    padding: 8px 4px;
    color: var(--fg-3);
    font-size: 11px;
    text-align: center;
    border: 1px dashed var(--border-2);
    border-radius: var(--radius);
  }

  .empty {
    padding: 32px 16px;
    color: var(--fg-3);
    font-size: 12px;
    text-align: center;
  }
  .empty.err { color: var(--crash); }
</style>
