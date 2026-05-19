<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    fleetBoard,
    fleetColumns,
    loadFleetBoard,
    moveLocalAcross,
    applyItem,
    removeItem,
    type FleetItem
  } from '$stores/fleet-board';
  import { sessions, loadSessions } from '$stores/sessions';
  import { watchdog, loadWatchdog } from '$stores/watchdog';
  import { profiles } from '$lib/profiles';
  import { actorId } from '$stores/actor';
  import { api, type BoardItem } from '$lib/api';
  import { deriveState, fmtTokens, fmtCost } from '$lib/dashboard';
  import Ticket from '$components/dashboard/Ticket.svelte';
  import Watchdog from '$components/dashboard/Watchdog.svelte';
  import BoardItemDialog from '$components/BoardItemDialog.svelte';

  // Safety-net refresh interval. The WS event bridge keeps the board
  // fresh on every board.* / session.* event; this only catches the
  // rare case where the socket is in long-running reconnect or a
  // non-active profile's bus has nothing to broadcast our way yet.
  const SAFETY_REFRESH_MS = 30_000;
  let pollId: ReturnType<typeof setInterval> | null = null;

  // Drag state is (profile_id, id) — IDs collide across servers, so a
  // simple number isn't enough. Keep them as separate fields rather
  // than a compound string to avoid churn in equality checks.
  let draggingProfileId = $state<string | null>(null);
  let draggingId = $state<number | null>(null);
  let dropTargetCol = $state<string | null>(null);

  // Dialog state. One component handles both create + edit; mode and
  // the bound item discriminate. Edits remember which profile owns the
  // ticket so the PATCH routes back to the same daemon.
  let dialogOpen = $state(false);
  let dialogMode = $state<'create' | 'edit'>('create');
  let dialogItem = $state<FleetItem | null>(null);
  let dialogDefaultStatus = $state<string | null>(null);
  let dialogProfileId = $state<string | null>(null);

  function refresh() {
    void loadFleetBoard();
    void loadSessions();
    void loadWatchdog(30);
  }

  onMount(() => {
    refresh();
    pollId = setInterval(refresh, SAFETY_REFRESH_MS);
  });
  onDestroy(() => { if (pollId) clearInterval(pollId); });

  // Show the per-card server chip only when more than one paired
  // profile exists. Single-server setups keep the original chrome.
  const showServerChip = $derived($profiles.length > 1);

  type ColView = {
    key: string;
    label: string;
    items: FleetItem[];
    tone: 'default' | 'live' | 'warn' | 'done';
  };
  const cols = $derived.by<ColView[]>(() =>
    $fleetColumns.map((c) => {
      const k = c.key.toLowerCase();
      let tone: ColView['tone'] = 'default';
      if (/claimed|progress/.test(k)) tone = 'live';
      else if (/review|pr/.test(k)) tone = 'warn';
      else if (/done|shipped|merged/.test(k)) tone = 'done';
      return { key: c.key, label: prettify(c.key), items: c.items, tone };
    })
  );

  function prettify(key: string): string {
    return key.replace(/[_-]/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
  }

  function profileLabelFor(id: string): string {
    const p = $profiles.find((x) => x.id === id);
    if (!p) return id;
    return p.baseUrl ? p.label : 'local';
  }

  /* -- drag and drop ------------------------------------------------- */
  type DragPayload = { profile_id: string; id: number };

  function onTicketDragStart(item: FleetItem) {
    return (e: DragEvent) => {
      draggingProfileId = item.profile_id;
      draggingId = item.id;
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = 'move';
        const payload: DragPayload = { profile_id: item.profile_id, id: item.id };
        e.dataTransfer.setData('application/json', JSON.stringify(payload));
      }
    };
  }
  function onTicketDragEnd() {
    draggingProfileId = null;
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
  /// "Active" columns (someone is/should-be working on it) vs "idle"
  /// (sitting in a queue). Used to drive auto-claim on drag.
  function isActiveColumn(key: string): boolean {
    const k = key.toLowerCase();
    if (/todo|backlog|inbox/.test(k)) return false;
    if (/done|shipped|merged|cancel/.test(k)) return false;
    return true; // doing / progress / review / etc.
  }

  function findItem(profileId: string, id: number): FleetItem | undefined {
    return $fleetBoard.items.find(
      (it) => it.profile_id === profileId && it.id === id
    );
  }

  function onColDrop(colKey: string) {
    return async (e: DragEvent) => {
      e.preventDefault();
      const raw = e.dataTransfer?.getData('application/json') ?? '';
      let payload: DragPayload | null = null;
      try { payload = JSON.parse(raw) as DragPayload; } catch { payload = null; }
      draggingProfileId = null;
      draggingId = null;
      dropTargetCol = null;
      if (!payload || !Number.isFinite(payload.id)) return;

      const item = findItem(payload.profile_id, payload.id);
      const fromActive = item ? isActiveColumn(item.status) : false;
      const toActive   = isActiveColumn(colKey);
      const me = actorId();

      // Optimistic local update first; reconcile on next fleet refresh.
      moveLocalAcross(payload.profile_id, payload.id, colKey);

      try {
        // Auto-claim when entering an active column from idle, if the
        // row is currently unclaimed. The user explicitly asked for
        // claim-on-status-change; this matches the "dragging into
        // doing implies I am taking it" intuition.
        if (item && !fromActive && toActive && item.claimed_by == null) {
          try {
            await api.claimBoardItemOn(payload.profile_id, item.id, me);
          } catch (claimErr) {
            // Claim conflict (someone else took it in the meantime)
            // shouldn't block the status move — log + carry on.
            console.warn('auto-claim conflict, continuing with status change:', claimErr);
          }
        }

        // Auto-release when dropping back into an idle column, but
        // only when *we* hold the claim. Foreign holders' claims are
        // left alone so a drag-by-someone-else doesn't release them.
        if (item && fromActive && !toActive && item.claimed_by === me) {
          try {
            await api.releaseBoardItemOn(payload.profile_id, item.id, me);
          } catch (releaseErr) {
            console.warn('auto-release failed, continuing:', releaseErr);
          }
        }

        await api.patchBoardItemOn(payload.profile_id, payload.id, { status: colKey });
      } catch (err) {
        console.error('move failed', err);
        await loadFleetBoard();
      }
    };
  }

  function openTicket(tk: FleetItem) {
    dialogMode = 'edit';
    dialogItem = tk;
    dialogDefaultStatus = null;
    dialogProfileId = tk.profile_id;
    dialogOpen = true;
  }

  function openCreate(status: string | null = null) {
    dialogMode = 'create';
    dialogItem = null;
    dialogDefaultStatus = status;
    // Default the new ticket to the active profile; user can re-pick.
    dialogProfileId = null;
    dialogOpen = true;
  }

  function closeDialog() {
    dialogOpen = false;
    dialogItem = null;
    dialogProfileId = null;
  }

  /// Server-confirmed create/update fires this callback synchronously
  /// (well before the WS-refetch lands). Splicing locally avoids the
  /// ~250 ms gap where the column would otherwise look stale.
  function onItemCreated(profileId: string, it: BoardItem) {
    applyItem(profileId, it);
  }
  function onItemUpdated(profileId: string, it: BoardItem) {
    applyItem(profileId, it);
  }
  function onItemDeleted(profileId: string, id: number) {
    removeItem(profileId, id);
  }

  /* -- right-rail metrics ------------------------------------------- */
  const live = $derived($sessions.items.filter((s) => deriveState(s) === 'live'));
  const claimedCount = $derived(
    $fleetBoard.items.filter((it) => /claimed|progress|review/i.test(it.status)).length
  );
  const reviewCount = $derived(
    $fleetBoard.items.filter((it) => /review|pr/i.test(it.status)).length
  );
  const doneCount = $derived(
    $fleetBoard.items.filter((it) => /done|shipped|merged/i.test(it.status)).length
  );
  const totalCount = $derived($fleetBoard.items.length);
  const tokens24 = $derived($sessions.items.reduce((a, s) => a + (s.tokens ?? 0), 0));
  const spend24  = $derived($sessions.items.reduce((a, s) => a + (s.cost ?? 0), 0));

  // Surfacing per-profile failures in the empty state so a single bad
  // endpoint (offline VPS, expired token) doesn't get silently swallowed.
  const profileErrors = $derived(Object.entries($fleetBoard.errors));
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
    <button type="button" class="tb-btn primary" onclick={() => openCreate(null)}>+ Ticket</button>
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

  <!-- Per-profile error banner — surfaces offline / unauth profiles so
       a partial fleet failure isn't silently hidden. Each profile gets
       its own pill so the user can see which endpoint is unhappy. -->
  {#if profileErrors.length > 0}
    <div class="fleet-errs">
      {#each profileErrors as [pid, msg] (pid)}
        <span class="err-pill" title={msg}>{profileLabelFor(pid)}: {msg}</span>
      {/each}
    </div>
  {/if}

  <!-- Board + rail -->
  <div class="row">
    <div class="board-wrap">
      {#if $fleetBoard.loading && totalCount === 0 && profileErrors.length === 0}
        <div class="empty mono">Loading board…</div>
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
                <button
                  type="button"
                  class="add"
                  aria-label={`Add to ${col.label}`}
                  onclick={() => openCreate(col.key)}
                >+</button>
              </div>
              <div class="col-b">
                {#each col.items as tk (`${tk.profile_id}:${tk.id}`)}
                  <Ticket
                    {tk}
                    sourceLabel={showServerChip ? profileLabelFor(tk.profile_id) : null}
                    dragging={draggingProfileId === tk.profile_id && draggingId === tk.id}
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

<BoardItemDialog
  open={dialogOpen}
  mode={dialogMode}
  item={dialogItem}
  defaultStatus={dialogDefaultStatus}
  defaultProfileId={dialogProfileId}
  columns={cols.map((c) => c.key)}
  onClose={closeDialog}
  onCreated={onItemCreated}
  onUpdated={onItemUpdated}
  onDeleted={onItemDeleted}
/>

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

  /* Per-profile failure pills — one per offline / unauth profile.
     Sits above the board so the rest of the fleet still renders. */
  .fleet-errs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--crash) 6%, var(--bg));
  }
  .err-pill {
    display: inline-flex;
    align-items: center;
    padding: 3px 8px;
    border-radius: var(--radius-pill);
    border: 1px solid color-mix(in srgb, var(--crash) 35%, var(--border-2));
    background: color-mix(in srgb, var(--crash) 10%, transparent);
    color: var(--crash);
    font-family: var(--mono);
    font-size: 10.5px;
    max-width: 360px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .empty {
    padding: 32px 16px;
    color: var(--fg-3);
    font-size: 12px;
    text-align: center;
  }

  /* Stack rail beneath the board on tablets — keep it accessible
     without crushing the columns. */
  @media (max-width: 1100px) {
    .row { flex-direction: column; }
    :global(.rail) {
      width: 100%;
      border-left: 0;
      border-top: 1px solid var(--border);
      max-height: 280px;
    }
  }

  /* Phone: collapse the kanban grid into a horizontal scroll-snap
     carousel so each column is reachable but the page doesn't try to
     squeeze 4 narrow columns into ~360px. */
  @media (max-width: 720px) {
    /* Sticky route header. */
    :global(.toolbar) {
      position: sticky;
      top: 0;
      z-index: 5;
      background: color-mix(in srgb, var(--bg-chrome) 92%, transparent);
      backdrop-filter: blur(10px);
      -webkit-backdrop-filter: blur(10px);
      padding: 10px 12px;
    }
    /* Hide the duplicate "+ Ticket" — bottom nav + (eventual) per-col
     "+" still allow creation. */
    :global(.toolbar .tb-btn.primary) { display: none; }
    /* Hide the meta "group: status" / "assignee" pills which take a
       full row but aren't actionable yet. */
    :global(.toolbar .pill) { display: none; }

    :global(.board) {
      display: flex !important;
      grid-template-columns: none !important;
      overflow-x: auto;
      overflow-y: hidden;
      scroll-snap-type: x mandatory;
      padding: 12px;
      gap: 10px;
      -webkit-overflow-scrolling: touch;
    }
    :global(.board .col) {
      flex: 0 0 84vw;
      min-width: 240px;
      max-width: 320px;
      scroll-snap-align: center;
    }

    /* Stack the rail under the board on phone, with bounded height so
       both rail content and board column carousel are reachable. */
    :global(.rail) {
      max-height: 320px;
    }

    .strip {
      padding: 8px 12px;
      gap: 6px;
      -webkit-overflow-scrolling: touch;
      scrollbar-width: none;
    }
    .strip::-webkit-scrollbar { display: none; }
    .strip .chip { padding: 7px 12px; font-size: 12px; }
  }
</style>
