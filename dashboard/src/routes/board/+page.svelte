<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    fleetBoard,
    fleetColumns,
    fleetLanes,
    loadFleetBoard,
    moveLocalAcross,
    patchStatusWithSnapBackOn,
    applyItem,
    removeItem,
    type FleetItem,
    type Lane
  } from '$stores/fleet-board';
  import type { RequiredField } from '$lib/board-schema';
  import { sessions, loadSessions } from '$stores/sessions';
  import { profiles } from '$lib/profiles';
  import { actorId } from '$stores/actor';
  import { api, type BoardItem } from '$lib/api';
  import { deriveState, fmtTokens, fmtCost } from '$lib/dashboard';
  import Ticket from '$components/dashboard/Ticket.svelte';
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
  /// Drop target key = `${profile_id}:${project}:${col}`. With multiple
  /// lanes on screen at once, a single col name isn't enough to
  /// identify which specific column got the hover.
  let dropTargetKey = $state<string | null>(null);
  /// When the drag is hovering above a specific ticket (within-column
  /// reorder), this is `${profile_id}:${id}` of that ticket. Renders
  /// an insertion line at the top of the target card.
  let dropAboveTicket = $state<string | null>(null);
  /// Collapsed lanes keyed by `${profile_id}:${project}`. Empty lanes
  /// stay expanded by default; the user can fold a noisy one.
  let collapsedLanes = $state<Set<string>>(new Set());
  /// Per-ticket comment count keyed by `${profile_id}:${id}`. Pulled
  /// from the fleet store, which fans out `/api/board` per profile —
  /// the response already includes `comment_counts` so the 💬N chip
  /// stays current without each card refetching.
  const commentCounts = $derived($fleetBoard.commentCounts);

  // Dialog state. One component handles both create + edit; mode and
  // the bound item discriminate. Edits remember which profile owns the
  // ticket so the PATCH routes back to the same daemon. A per-lane
  // quick-add seeds defaultWorkdir so the new ticket lands in the
  // lane the user clicked + on.
  let dialogOpen = $state(false);
  let dialogMode = $state<'create' | 'edit'>('create');
  let dialogItem = $state<FleetItem | null>(null);
  let dialogDefaultStatus = $state<string | null>(null);
  let dialogProfileId = $state<string | null>(null);
  let dialogDefaultWorkdir = $state<string | null>(null);
  /// Server-rejected fields from the last drag-drop snap-back. Seeded
  /// into the dialog on `openDialogForRejection` so the user sees the
  /// red borders on the inputs that need to be filled before the move
  /// will be accepted. Cleared by `closeDialog` and by any non-rejection
  /// open.
  let dialogInitialMissing = $state<RequiredField[]>([]);

  function laneKey(profileId: string, project: string): string {
    return `${profileId}::${project}`;
  }
  function toggleLane(key: string) {
    const next = new Set(collapsedLanes);
    if (next.has(key)) next.delete(key); else next.add(key);
    collapsedLanes = next;
  }

  function refresh() {
    void loadFleetBoard();
    void loadSessions();
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
    dropTargetKey = null;
    dropAboveTicket = null;
  }
  function onColDragOver(key: string, foreignProfile: boolean) {
    return (e: DragEvent) => {
      if (draggingId == null) return;
      // Cross-profile drops aren't supported (would require copy+delete
      // across daemons). Refuse the drag so the cursor reads "no-drop".
      if (foreignProfile) {
        if (e.dataTransfer) e.dataTransfer.dropEffect = 'none';
        return;
      }
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
      dropTargetKey = key;
    };
  }
  function onColDragLeave(key: string) {
    return () => { if (dropTargetKey === key) dropTargetKey = null; };
  }

  /// Card-level dragover: paints an insertion line above the hovered
  /// target card so the user can see where the drop will land. The
  /// column-level handler still runs (Svelte event bubbling) so
  /// dropTargetKey stays in sync for the column highlight.
  function onTicketDragOver(item: FleetItem, foreignProfile: boolean) {
    return (e: DragEvent) => {
      if (draggingId == null) return;
      if (foreignProfile) return;
      // Don't paint a line above the card being dragged itself.
      if (draggingProfileId === item.profile_id && draggingId === item.id) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
      dropAboveTicket = `${item.profile_id}:${item.id}`;
    };
  }
  function onTicketDragLeave(item: FleetItem) {
    return () => {
      const key = `${item.profile_id}:${item.id}`;
      if (dropAboveTicket === key) dropAboveTicket = null;
    };
  }

  /// Card-level drop: insert the dragged ticket above the target,
  /// recompute priorities for the affected column, and send one batch.
  function onTicketDrop(lane: Lane, colKey: string, target: FleetItem) {
    return async (e: DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const raw = e.dataTransfer?.getData('application/json') ?? '';
      let payload: { profile_id: string; id: number } | null = null;
      try { payload = JSON.parse(raw); } catch { payload = null; }
      dropTargetKey = null;
      dropAboveTicket = null;
      draggingProfileId = null;
      draggingId = null;
      if (!payload || payload.profile_id !== lane.profile_id) return;

      const item = findItem(payload.profile_id, payload.id);
      if (!item) return;

      // Capture origin column BEFORE the optimistic local apply below so
      // the snap-back helper has a real starting status to revert to. If
      // we read item.status after the applyItemLocal loop, origin ===
      // target and snap-back becomes a no-op — the same bug shape we
      // had to fix in onColDrop.
      const originStatus = item.status;

      // Same-column reorder: rewrite priorities so the dragged item
      // lands above `target`. The store renumbers all rows in one tx.
      const currentColItems = (lane.byStatus[colKey] ?? [])
        .filter((it) => it.id !== item.id);
      const targetIdx = currentColItems.findIndex((it) => it.id === target.id);
      const insertAt = targetIdx < 0 ? currentColItems.length : targetIdx;
      const reordered = [
        ...currentColItems.slice(0, insertAt),
        { ...item, status: colKey, workdir: lane.workdir ?? item.workdir },
        ...currentColItems.slice(insertAt)
      ];

      // Optimistic local apply of the new order (status + workdir +
      // priority). Priorities are 10-spaced so subsequent inserts
      // between two rows don't immediately need a renumber.
      const entries = reordered.map((it, i) => ({ id: it.id, priority: i * 10 }));
      for (const it of reordered) applyItemLocal(it as FleetItem);

      const fromActive = isActiveColumn(originStatus);
      const toActive = isActiveColumn(colKey);
      const me = actorId();
      try {
        if (!fromActive && toActive && item.claimed_by == null) {
          try { await api.claimBoardItemOn(payload.profile_id, item.id, me); } catch (err) {
            console.warn('auto-claim conflict during reorder:', err);
          }
        }
        if (fromActive && !toActive && item.claimed_by === me) {
          try { await api.releaseBoardItemOn(payload.profile_id, item.id, me); } catch (err) {
            console.warn('auto-release failed during reorder:', err);
          }
        }
        // Cross-column moves still need the status + workdir patch.
        // Route through patchStatusWithSnapBackOn (the same helper
        // onColDrop uses) so a 400 gate rejection reverts the optimistic
        // move and reopens the dialog pre-highlighted on the missing
        // fields. Same-column drops skip this branch entirely; the
        // reorder follow-up is filed separately.
        if (originStatus !== colKey || (lane.workdir != null && item.workdir !== lane.workdir)) {
          const workdirPatch: BoardPatchLite =
            lane.workdir != null && item.workdir !== lane.workdir
              ? { workdir: lane.workdir }
              : {};
          const rejection = await patchStatusWithSnapBackOn(
            payload.profile_id,
            item.id,
            originStatus,
            colKey,
            workdirPatch
          );
          if (rejection) {
            // Snap-back already reverted the dragged item's status
            // locally; skip the reorder batch since the card isn't
            // landing in this column. The dialog gives the user a
            // path to fill in the missing fields and try again.
            openDialogForRejection(item, rejection.missing);
            return;
          }
        }
        await api.reorderBoardOn(payload.profile_id, entries);
      } catch (err) {
        console.error('reorder failed', err);
        await loadFleetBoard();
      }
    };
  }

  function applyItemLocal(it: FleetItem) {
    applyItem(it.profile_id, it);
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

  /// Drop into a (lane, column). When the drop lane's workdir differs
  /// from the item's current workdir, the PATCH bundles both fields so
  /// drag-to-organize works as a single op.
  function onColDrop(lane: Lane, colKey: string) {
    return async (e: DragEvent) => {
      e.preventDefault();
      const raw = e.dataTransfer?.getData('application/json') ?? '';
      let payload: DragPayload | null = null;
      try { payload = JSON.parse(raw) as DragPayload; } catch { payload = null; }
      draggingProfileId = null;
      draggingId = null;
      dropTargetKey = null;
      if (!payload || !Number.isFinite(payload.id)) return;

      // Cross-profile drops aren't supported — they'd require copy +
      // delete across daemons. Drag-out within the same server is the
      // only path; users can still re-create on a different server via
      // the dialog's Servers tile.
      if (payload.profile_id !== lane.profile_id) {
        console.warn('cross-profile drop ignored');
        return;
      }

      const item = findItem(payload.profile_id, payload.id);
      // Capture the origin column BEFORE the optimistic move so the
      // snap-back helper has a real starting status to revert to. If
      // we read item.status after moveLocalAcross, origin === target
      // and snap-back becomes a no-op — the most common bug shape.
      const originStatus = item?.status ?? null;
      const fromActive = item ? isActiveColumn(item.status) : false;
      const toActive   = isActiveColumn(colKey);
      const me = actorId();

      // Optimistic local update first; reconcile on next fleet refresh.
      moveLocalAcross(payload.profile_id, payload.id, colKey);

      try {
        // Auto-claim when entering an active column from idle, if the
        // row is currently unclaimed. The user explicitly asked for
        // claim-on-status-change. Claim doesn't change status, so a
        // claimed-but-snapped-back card is fine — no undo needed when
        // the subsequent PATCH gets rejected.
        if (item && !fromActive && toActive && item.claimed_by == null) {
          try {
            await api.claimBoardItemOn(payload.profile_id, item.id, me);
          } catch (claimErr) {
            console.warn('auto-claim conflict, continuing with status change:', claimErr);
          }
        }

        if (item && fromActive && !toActive && item.claimed_by === me) {
          try {
            await api.releaseBoardItemOn(payload.profile_id, item.id, me);
          } catch (releaseErr) {
            console.warn('auto-release failed, continuing:', releaseErr);
          }
        }

        // Cross-lane drop: also re-home the ticket to the target lane's
        // workdir so dragging is a drag-to-organize op, not just a
        // status-only move. The snap-back helper folds these extra
        // fields into the same PATCH body.
        const workdirPatch: BoardPatchLite =
          item && lane.workdir != null && item.workdir !== lane.workdir
            ? { workdir: lane.workdir }
            : {};

        // Route through the snap-back helper. On a 400 gate rejection
        // it reverts moveLocalAcross() and returns the missing fields
        // so we can reopen the edit dialog pre-highlighted. Origin
        // fallback to the current dragging status if findItem missed
        // (shouldn't happen, but defensive).
        const rejection = originStatus != null
          ? await patchStatusWithSnapBackOn(
              payload.profile_id,
              payload.id,
              originStatus,
              colKey,
              workdirPatch
            )
          : null;
        if (rejection && item) {
          openDialogForRejection(item, rejection.missing);
        }
      } catch (err) {
        console.error('move failed', err);
        await loadFleetBoard();
      }
    };
  }

  /// Reopen the edit dialog for a ticket whose drag-drop status change
  /// was rejected by the server's gate. The dialog seeds `rejectedFields`
  /// from `missing` so the inputs render with the red borders the user
  /// needs to address before retrying the move.
  function openDialogForRejection(item: FleetItem, missing: RequiredField[]) {
    dialogMode = 'edit';
    dialogItem = item;
    dialogDefaultStatus = null;
    dialogProfileId = item.profile_id;
    dialogInitialMissing = missing;
    dialogOpen = true;
  }

  /// Local alias so the lane-aware drop handler can declare a partial
  /// BoardPatch without a wider import surface.
  type BoardPatchLite = { status?: string; workdir?: string | null };

  function openTicket(tk: FleetItem) {
    dialogMode = 'edit';
    dialogItem = tk;
    dialogDefaultStatus = null;
    dialogProfileId = tk.profile_id;
    dialogInitialMissing = [];
    dialogOpen = true;
  }

  function openCreate(
    status: string | null = null,
    profileId: string | null = null,
    workdir: string | null = null
  ) {
    dialogMode = 'create';
    dialogItem = null;
    dialogDefaultStatus = status;
    // Per-lane quick-add seeds both profile + workdir so the new
    // ticket lands in the lane the user clicked +.
    dialogProfileId = profileId;
    dialogDefaultWorkdir = workdir;
    dialogInitialMissing = [];
    dialogOpen = true;
  }

  function closeDialog() {
    dialogOpen = false;
    dialogItem = null;
    dialogProfileId = null;
    dialogDefaultWorkdir = null;
    dialogInitialMissing = [];
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
      <button type="button" class="tab active">Board <span class="badge">{totalCount}</span></button>
      <a class="tab" href="/sessions">Sessions <span class="badge">{$sessions.items.length}</span></a>
    </div>
    <span class="spacer"></span>
    <span class="pill"><span style="color: var(--fg-3);">group:</span>&nbsp;server · project</span>
    <button type="button" class="tb-btn primary" onclick={() => openCreate(null)}>+ Ticket</button>
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
      {:else if $fleetLanes.length === 0}
        <div class="empty mono">
          <p>No tickets yet. Press <kbd>+ Ticket</kbd> to seed the board.</p>
        </div>
      {:else}
        <div class="lanes">
          {#each $fleetLanes as lane (laneKey(lane.profile_id, lane.project))}
            {@const lkey = laneKey(lane.profile_id, lane.project)}
            {@const collapsed = collapsedLanes.has(lkey)}
            <section class="lane" class:collapsed>
              <header class="lane-h">
                <button
                  type="button"
                  class="lane-toggle"
                  aria-expanded={!collapsed}
                  onclick={() => toggleLane(lkey)}
                  title={collapsed ? 'Expand lane' : 'Collapse lane'}
                >
                  <span class="caret">{collapsed ? '▸' : '▾'}</span>
                </button>
                <span class="lane-server" title={lane.profile_id}>@{lane.profile_label}</span>
                <span class="lane-sep">/</span>
                <span class="lane-project" title={lane.workdir ?? '(no workdir)'}>{lane.project}</span>
                <span class="lane-count">{lane.total}</span>
                <span class="lane-spacer"></span>
                <button
                  type="button"
                  class="lane-add"
                  onclick={() => openCreate(null, lane.profile_id, lane.workdir)}
                  title="Add a ticket to this project"
                >+ Ticket</button>
              </header>
              {#if !collapsed}
                <div class="board" style:grid-template-columns={`repeat(${Math.min(cols.length, 4)}, minmax(0, 1fr))`}>
                  {#each cols as col (col.key)}
                    {@const dk = `${lkey}:${col.key}`}
                    {@const foreign = draggingProfileId != null && draggingProfileId !== lane.profile_id}
                    <div
                      class="col"
                      class:drop-target={dropTargetKey === dk}
                      class:foreign={foreign && draggingId != null}
                      ondragover={onColDragOver(dk, foreign)}
                      ondragleave={onColDragLeave(dk)}
                      ondrop={onColDrop(lane, col.key)}
                      role="region"
                      aria-label={`${col.label} — ${lane.project}`}
                    >
                      <div class="col-h">
                        {#if col.tone === 'live'}<span style="color: var(--green);">●</span>
                        {:else if col.tone === 'warn'}<span style="color: var(--amber);">●</span>
                        {:else if col.tone === 'done'}<span style="color: var(--fg-3);">●</span>{/if}
                        <span>{col.label}</span>
                        <span class="count">{(lane.byStatus[col.key] ?? []).length}</span>
                        <button
                          type="button"
                          class="add"
                          aria-label={`Add ${col.label} to ${lane.project}`}
                          onclick={() => openCreate(col.key, lane.profile_id, lane.workdir)}
                        >+</button>
                      </div>
                      <div class="col-b">
                        {#each (lane.byStatus[col.key] ?? []) as tk (`${tk.profile_id}:${tk.id}`)}
                          {@const tkKey = `${tk.profile_id}:${tk.id}`}
                          {@const tkForeign = draggingProfileId != null && draggingProfileId !== tk.profile_id}
                          <Ticket
                            {tk}
                            sourceLabel={showServerChip ? profileLabelFor(tk.profile_id) : null}
                            commentCount={commentCounts[tkKey] ?? 0}
                            dropAbove={dropAboveTicket === tkKey}
                            dragging={draggingProfileId === tk.profile_id && draggingId === tk.id}
                            onDragStart={onTicketDragStart(tk)}
                            onDragEnd={onTicketDragEnd}
                            onDragOver={onTicketDragOver(tk, tkForeign)}
                            onDragLeave={onTicketDragLeave(tk)}
                            onDrop={onTicketDrop(lane, col.key, tk)}
                            onClick={() => openTicket(tk)}
                          />
                        {/each}
                        {#if (lane.byStatus[col.key] ?? []).length === 0}
                          <div class="col-empty mono">empty</div>
                        {/if}
                      </div>
                    </div>
                  {/each}
                </div>
              {/if}
            </section>
          {/each}
        </div>
      {/if}
    </div>

    <aside class="rail" style="width: 300px;">
      <div class="rh">
        <span>burn-down</span>
        <span class="spacer"></span>
        <span style={doneCount >= claimedCount ? 'color: var(--green);' : 'color: var(--amber);'}>
          {doneCount >= claimedCount ? 'on track' : 'behind'}
        </span>
      </div>
      <div class="rb">
        <div class="group">
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
  defaultWorkdir={dialogDefaultWorkdir}
  initialMissing={dialogInitialMissing}
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
  .col-empty {
    padding: 8px 4px;
    color: var(--fg-3);
    font-size: 11px;
    text-align: center;
    border: 1px dashed var(--border-2);
    border-radius: var(--radius);
  }

  /* Stacked swimlanes — one per (server, project). Each lane is a
     mini-board so the user can scan everything at once but visually
     separate concerns. */
  .lanes {
    flex: 1;
    overflow-y: auto;
    padding: 12px 14px 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .lane {
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    overflow: hidden;
  }
  .lane-h {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: color-mix(in srgb, var(--bg-2) 75%, transparent);
    border-bottom: 1px solid var(--border);
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--fg-2);
  }
  .lane.collapsed .lane-h { border-bottom: 0; }
  .lane-toggle {
    background: none;
    border: 0;
    padding: 0 2px;
    color: var(--fg-3);
    cursor: pointer;
    line-height: 1;
    transition: color var(--t-hover);
  }
  .lane-toggle:hover { color: var(--fg); }
  .lane-toggle .caret { font-size: 10px; }
  .lane-server {
    color: var(--cta);
    letter-spacing: -0.005em;
  }
  .lane-sep { color: var(--fg-3); }
  .lane-project {
    color: var(--fg);
    font-weight: 500;
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lane-count {
    padding: 1px 7px;
    border-radius: var(--radius-pill);
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    color: var(--fg-3);
    font-size: 10.5px;
  }
  .lane-spacer { flex: 1; }
  .lane-add {
    padding: 4px 10px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border-2);
    background: transparent;
    color: var(--fg-2);
    font-family: var(--mono);
    font-size: 10.5px;
    cursor: pointer;
    transition: border-color var(--t-hover), color var(--t-hover), background var(--t-hover);
  }
  .lane-add:hover { border-color: var(--cta); color: var(--fg); background: color-mix(in srgb, var(--cta) 8%, transparent); }

  /* Cross-profile drag visibly refuses — drop targets dim when the
     source profile differs. */
  :global(.lane .col.foreign) { opacity: 0.45; }

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

  }
</style>
