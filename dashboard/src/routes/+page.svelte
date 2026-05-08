<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { sessions, loadSessions } from '$stores/sessions';
  import { board, loadBoard } from '$stores/board';
  import { watchdog, loadWatchdog } from '$stores/watchdog';
  import { openNewSession } from '$stores/newSession';
  import { api } from '$lib/api';
  import {
    deriveState, ctxOf, fmtTokens, fmtCost, greeting
  } from '$lib/dashboard';
  import SummaryCard from '$components/dashboard/SummaryCard.svelte';
  import NarrativeRow from '$components/dashboard/NarrativeRow.svelte';
  import FleetRow from '$components/dashboard/FleetRow.svelte';
  import AttentionStrip from '$components/dashboard/AttentionStrip.svelte';
  import HostStrip from '$components/dashboard/HostStrip.svelte';
  import StuckPanel from '$components/dashboard/StuckPanel.svelte';
  import { tweaks } from '$stores/tweaks';
  import { awaitingInput, staleMinutes } from '$stores/attention';
  import { projectOf } from '$lib/dashboard';
  import type { Session } from '$lib/api';

  let username = $state('there');

  type FleetFilter = 'all' | 'live' | 'attention' | 'stopped';
  type FleetSort = 'activity' | 'name' | 'ctx' | 'spend';
  type FleetGroup = 'none' | 'project' | 'status';
  let filterBy = $state<FleetFilter>('all');
  let sortBy   = $state<FleetSort>('activity');
  let groupBy  = $state<FleetGroup>('project');

  function refresh() {
    loadSessions();
    loadBoard();
    loadWatchdog(20);
  }

  onMount(() => {
    refresh();
    api.me().then(m => { if (m?.username) username = m.username; }).catch(() => {});
    const id = setInterval(refresh, 5000);
    return () => clearInterval(id);
  });

  const live      = $derived($sessions.items.filter(s => deriveState(s) === 'live'));
  const incidents = $derived(
    $sessions.items.filter(s => {
      const st = deriveState(s);
      return st === 'crash' || st === 'compact';
    })
  );
  const lowCtx    = $derived(
    $sessions.items
      .filter(s => deriveState(s) === 'live' && ctxOf(s) <= 55)
      .sort((a, b) => ctxOf(a) - ctxOf(b))
  );
  const avgCtx    = $derived(
    live.length === 0 ? 0 : Math.round(live.reduce((a, s) => a + ctxOf(s), 0) / live.length)
  );

  const tokens24  = $derived($sessions.items.reduce((a, s) => a + (s.tokens ?? 0), 0));
  const spend24   = $derived($sessions.items.reduce((a, s) => a + (s.cost ?? 0), 0));
  const stoppedCount = $derived($sessions.items.filter(s => s.status === 'stopped').length);
  const compactingCount = $derived(incidents.filter(i => deriveState(i) === 'compact').length);
  const crashedCount = $derived(incidents.filter(i => deriveState(i) === 'crash').length);
  const stuckCount = $derived.by(() => {
    const lim = $tweaks.stuckMinutes;
    let n = 0;
    for (const s of $sessions.items) {
      if (s.status !== 'running') continue;
      if ($awaitingInput.has(s.id)) { n += 1; continue; }
      if (deriveState(s) === 'idle' && staleMinutes(s) >= lim) n += 1;
    }
    return n;
  });

  // Filter → sort → group pipeline. Pinned always wins regardless of
  // sort field so favorited rows stay at the top of every grouping.
  const filteredSessions = $derived.by(() => {
    const lim = $tweaks.stuckMinutes;
    const isAttention = (s: Session) =>
      $awaitingInput.has(s.id)
      || deriveState(s) === 'crash'
      || deriveState(s) === 'compact'
      || (s.status === 'running' && deriveState(s) === 'idle' && staleMinutes(s) >= lim);
    return $sessions.items.filter((s) => {
      switch (filterBy) {
        case 'live':      return deriveState(s) === 'live';
        case 'attention': return isAttention(s);
        case 'stopped':   return s.status === 'stopped' || s.status === 'crashed';
        default:          return true;
      }
    });
  });

  const sortedSessions = $derived.by(() => {
    const arr = [...filteredSessions];
    arr.sort((a, b) => {
      // Pinned first regardless of secondary sort.
      const ap = a.pinned ? 1 : 0;
      const bp = b.pinned ? 1 : 0;
      if (ap !== bp) return bp - ap;
      switch (sortBy) {
        case 'name':     return a.name.localeCompare(b.name);
        case 'ctx':      return ctxOf(a) - ctxOf(b);
        case 'spend':    return (b.cost ?? 0) - (a.cost ?? 0);
        case 'activity':
        default: {
          const at = a.last_activity_at ? new Date(a.last_activity_at).getTime() : 0;
          const bt = b.last_activity_at ? new Date(b.last_activity_at).getTime() : 0;
          return bt - at;
        }
      }
    });
    return arr;
  });

  // Grouped view: array of [groupLabel, sessions[]]. `none` returns a
  // single bucket so the template always iterates over groups.
  const groupedSessions = $derived.by((): Array<[string, Session[]]> => {
    if (groupBy === 'none') return [['', sortedSessions]];
    const map = new Map<string, Session[]>();
    const keyFor = (s: Session) => {
      if (groupBy === 'project') return projectOf(s.workdir);
      // status grouping ladders by lifecycle so live sessions land first.
      const st = deriveState(s);
      if ($awaitingInput.has(s.id)) return 'attention';
      if (st === 'crash') return 'crashed';
      if (st === 'compact') return 'compacting';
      if (st === 'live') return 'live';
      if (s.status === 'stopped') return 'stopped';
      return 'idle';
    };
    for (const s of sortedSessions) {
      const k = keyFor(s);
      const arr = map.get(k) ?? [];
      arr.push(s);
      map.set(k, arr);
    }
    // Preserve insertion order (sortedSessions is already in the right
    // order, so first-seen-key is the right group order too).
    return Array.from(map.entries());
  });

  // Most-active project last 24h, by session count. Ties broken by
  // total spend so the "expensive workdir" surfaces over equal-count
  // ones. Returns null when there's no fleet to inspect.
  const topProject = $derived.by(() => {
    if ($sessions.items.length === 0) return null;
    const buckets = new Map<string, { count: number; spend: number; sessions: Session[] }>();
    for (const s of $sessions.items) {
      const k = projectOf(s.workdir);
      const b = buckets.get(k) ?? { count: 0, spend: 0, sessions: [] };
      b.count += 1;
      b.spend += s.cost ?? 0;
      b.sessions.push(s);
      buckets.set(k, b);
    }
    let best: { name: string; count: number; spend: number; sessions: Session[] } | null = null;
    for (const [name, b] of buckets) {
      if (!best || b.count > best.count || (b.count === best.count && b.spend > best.spend)) {
        best = { name, ...b };
      }
    }
    return best;
  });

  const doneItems = $derived.by(() => {
    const data = $board.data;
    if (!data) return [];
    const doneKey = data.column_order.find(k => /done|shipped|merged/i.test(k));
    return doneKey ? data.columns[doneKey] ?? [] : [];
  });
  const reviewItems = $derived.by(() => {
    const data = $board.data;
    if (!data) return [];
    const reviewKey = data.column_order.find(k => /review|pr/i.test(k));
    return reviewKey ? data.columns[reviewKey] ?? [] : [];
  });

  const now = new Date();
  const dateLine = $derived(
    `${now.toLocaleDateString('en-US', { weekday: 'long' })}, ${now.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' })} · ${live.length} session${live.length === 1 ? '' : 's'} running`
  );

  async function triageIncidents() {
    if (incidents[0]) goto(`/sessions/${incidents[0].id}`);
  }

  async function compactSession(id: string) {
    try {
      await api.sendInput(id, { keys: '/compact', append_enter: true });
    } catch (e) { console.error(e); }
  }
</script>

<div class="page">
  <div class="toolbar">
    <span class="micro" style="color: var(--fg-2);">Overview</span>
    <span class="micro" style="margin-left: 4px;">· last 24h</span>
    <span class="spacer"></span>
    <button type="button" class="tb-btn primary" onclick={openNewSession}>
      <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
        <path d="M3 8h10M8 3v10" stroke-linecap="round"/>
      </svg>
      Spawn session
    </button>
  </div>

  <div class="scroll">
    <section class="hero">
      <div class="hero-text">
        <div class="micro" style="margin-bottom: 6px;">{dateLine}</div>
        <h1 class="hello">{greeting()}, {username}.</h1>
        <div class="narrative">
          {#if doneItems.length > 0}
            <NarrativeRow
              icon="✓"
              tone="ok"
              lead={`${doneItems.length} ticket${doneItems.length === 1 ? '' : 's'}`}
              body=" shipped in the last 24h."
            />
          {/if}
          {#if incidents.length > 0}
            <NarrativeRow
              icon="!"
              tone={incidents.some(i => deriveState(i) === 'crash') ? 'crash' : 'warn'}
              lead={`${incidents.length} agent${incidents.length === 1 ? '' : 's'}`}
              body={
                ' need attention: ' +
                incidents.map(i => `${i.name} ${deriveState(i) === 'crash' ? 'crashed' : 'compacting'}`).join(', ') + '.'
              }
            />
          {/if}
          {#if lowCtx.length > 0}
            <NarrativeRow
              icon="◐"
              tone="info"
              lead={lowCtx[0].name}
              body={` is at ${ctxOf(lowCtx[0])}% context — likely needs /compact in the next ${ctxOf(lowCtx[0]) > 50 ? '20–30' : '10'} minutes.`}
            />
          {/if}
          {#if doneItems.length === 0 && incidents.length === 0 && lowCtx.length === 0}
            <NarrativeRow
              icon="•"
              tone="info"
              lead="Quiet."
              body=" Fleet is healthy, nothing pending review."
            />
          {/if}
        </div>
        {#if incidents.length > 0}
          <div class="hero-actions">
            <button type="button" class="tb-btn primary" onclick={triageIncidents}>
              Triage incidents
              <span class="incident-count">{incidents.length}</span>
            </button>
          </div>
        {/if}
      </div>

      <div class="hero-side">
        {#if !$tweaks.hideHostStrip}
          <HostStrip />
        {/if}
      </div>
    </section>

    <div class="summary">
      <SummaryCard
        k="Fleet"
        v={String(live.length)}
        accent={stuckCount + crashedCount > 0 ? 'var(--cta)' : 'var(--green)'}
        tags={[
          { label: `${$sessions.items.length} total`, color: 'var(--fg-3)' },
          ...(stuckCount > 0 ? [{ label: `${stuckCount} stuck`, color: 'var(--amber)' }] : []),
          ...(crashedCount > 0 ? [{ label: `${crashedCount} crashed`, color: 'var(--crash)' }] : []),
          ...(stoppedCount > 0 ? [{ label: `${stoppedCount} stopped`, color: 'var(--fg-3)' }] : [])
        ]}
        foot={live.length > 0 ? 'streaming now' : 'no agents running'}
      />

      <SummaryCard
        k="Context"
        v={live.length > 0 ? `${avgCtx}%` : '—'}
        accent={live.length === 0 ? undefined : (avgCtx >= 70 ? 'var(--green)' : avgCtx >= 50 ? 'var(--amber)' : 'var(--cta)')}
        tags={[
          ...(lowCtx.length > 0
            ? [{ label: `${lowCtx.length} below 55%`, color: 'var(--cta)' }]
            : live.length > 0 ? [{ label: 'all healthy', color: 'var(--green)' }] : []),
          ...(compactingCount > 0 ? [{ label: `${compactingCount} compacting`, color: 'var(--amber)' }] : [])
        ]}
        foot={lowCtx[0] ? `${lowCtx[0].name} lowest at ${ctxOf(lowCtx[0])}%` : 'avg across live sessions'}
      />

      <SummaryCard
        k="Spend · 24h"
        v={fmtCost(spend24)}
        accent={spend24 > 0 ? 'var(--link)' : undefined}
        tags={[
          { label: `${fmtTokens(tokens24)} tokens` },
          ...(doneItems.length > 0 ? [{ label: `${doneItems.length} shipped`, color: 'var(--green)' }] : []),
          ...(reviewItems.length > 0 ? [{ label: `${reviewItems.length} in review`, color: 'var(--amber)' }] : [])
        ]}
        foot={doneItems.length > 0 ? `${fmtCost(spend24 / doneItems.length)} / shipped ticket` : (topProject ? `most active: ${topProject.name} · ${topProject.count} sess` : '—')}
      />
    </div>

    <StuckPanel stuckMinutes={$tweaks.stuckMinutes} />

    <section class="fleet">
      <div class="fleet-h">
        <span class="micro" style="color: var(--fg);">Fleet</span>
        <span class="micro">· {filteredSessions.length} of {$sessions.items.length} · {live.length} streaming</span>
        <span class="spacer"></span>
        <div class="seg" role="tablist" aria-label="Filter">
          {#each [['all','All'], ['live','Live'], ['attention','Attention'], ['stopped','Stopped']] as [v, label]}
            <button type="button" class="seg-btn" class:on={filterBy === v} onclick={() => filterBy = v as FleetFilter}>{label}</button>
          {/each}
        </div>
        <div class="seg" role="tablist" aria-label="Sort">
          {#each [['activity','Recent'], ['name','Name'], ['ctx','Ctx'], ['spend','Spend']] as [v, label]}
            <button type="button" class="seg-btn" class:on={sortBy === v} onclick={() => sortBy = v as FleetSort}>{label}</button>
          {/each}
        </div>
        <div class="seg" role="tablist" aria-label="Group">
          {#each [['none','Flat'], ['project','Project'], ['status','Status']] as [v, label]}
            <button type="button" class="seg-btn" class:on={groupBy === v} onclick={() => groupBy = v as FleetGroup}>{label}</button>
          {/each}
        </div>
      </div>

      {#if incidents.length + reviewItems.length > 0}
        <div class="strips">
          {#each incidents as s (s.id)}
            <AttentionStrip
              tone={deriveState(s) === 'crash' ? 'crash' : 'warn'}
              label={deriveState(s) === 'crash' ? 'crashed' : 'compacting'}
              target={s.name}
              detail={
                deriveState(s) === 'crash'
                  ? 'pane killed · auto-restart pending'
                  : `ctx ${ctxOf(s)}% · /compact recommended`
              }
              actionLabel={deriveState(s) === 'crash' ? 'Inspect' : 'Send /compact'}
              onAction={() => deriveState(s) === 'crash' ? goto(`/sessions/${s.id}`) : compactSession(s.id)}
            />
          {/each}
          {#each reviewItems.slice(0, 1) as tk (tk.id)}
            <AttentionStrip
              tone="amber"
              label="review ready"
              target={tk.key}
              detail={tk.title}
              actionLabel="Open diff"
              onAction={() => goto('/board')}
            />
          {/each}
        </div>
      {/if}

      <div class="fleet-head">
        <span></span>
        <span></span>
        <span>Session · task</span>
        <span>Last activity</span>
        <span style="text-align: right;">Tokens</span>
        <span style="text-align: right;">Cost</span>
        <span style="text-align: right;">Context</span>
        <span></span>
      </div>

      {#if $sessions.loading && $sessions.items.length === 0}
        <div class="empty mono">Loading fleet…</div>
      {:else if $sessions.error}
        <div class="empty mono err">Failed to load: {$sessions.error}</div>
      {:else if $sessions.items.length === 0}
        <div class="empty mono">
          No sessions yet — <button type="button" class="link" onclick={openNewSession}>spawn one</button>.
        </div>
      {:else if filteredSessions.length === 0}
        <div class="empty mono">No sessions match this filter.</div>
      {:else}
        {#each groupedSessions as [groupLabel, rows] (groupLabel || 'flat')}
          {#if groupLabel}
            <div class="group-h">
              <span class="g-name">{groupLabel}</span>
              <span class="g-count">{rows.length}</span>
            </div>
          {/if}
          {#each rows as s (s.id)}
            <FleetRow {s} />
          {/each}
        {/each}
      {/if}

      {#if doneItems.length > 0}
        <div class="just-shipped">
          <span class="micro">Just shipped</span>
          {#each doneItems.slice(0, 3) as tk (tk.id)}
            <span class="chip">
              <span class="chip-dot"></span>
              <span class="chip-k">{tk.key}</span>
              <span class="chip-t">{tk.title}</span>
              {#if tk.claimed_by}<span class="chip-who">{tk.claimed_by}</span>{/if}
            </span>
          {/each}
          <span class="spacer"></span>
          <a class="micro view-all" href="/board">view all →</a>
        </div>
      {/if}
    </section>
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
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 18px 18px 14px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .hero {
    display: grid;
    grid-template-columns: 1.2fr 1fr;
    gap: 18px;
    align-items: stretch;
  }
  .hero-text { display: flex; flex-direction: column; gap: 14px; }
  .hello {
    margin: 0;
    font-size: 30px;
    letter-spacing: -0.025em;
    font-weight: 500;
    line-height: 1.05;
    color: var(--fg);
  }
  .narrative {
    margin-top: 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .hero-actions {
    display: flex;
    gap: 8px;
    margin-top: auto;
  }
  .incident-count {
    margin-left: 6px;
    padding: 1px 5px;
    border-radius: var(--radius-pill);
    background: rgba(255, 255, 255, 0.18);
    font-size: 10px;
  }
  .hero-side {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: 12px;
    min-width: 0;
  }
  .summary {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
  }

  .fleet {
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: 10px;
    overflow: hidden;
  }
  .fleet-h {
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .spacer { flex: 1; }
  .fleet-h .seg {
    display: inline-flex;
    border: 1px solid var(--border-2);
    border-radius: 999px;
    overflow: hidden;
    background: var(--bg-2);
  }
  .fleet-h .seg-btn {
    background: transparent;
    border: 0;
    color: var(--fg-3);
    font-family: var(--mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 4px 10px;
    cursor: pointer;
    border-right: 1px solid var(--border-2);
    transition: color var(--t-hover), background var(--t-hover);
  }
  .fleet-h .seg-btn:last-child { border-right: 0; }
  .fleet-h .seg-btn:hover { color: var(--fg-2); }
  .fleet-h .seg-btn.on {
    background: var(--surface);
    color: var(--fg);
  }

  .group-h {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px 4px;
    background: #0c0c0c;
    border-bottom: 1px solid var(--border);
    font-family: var(--mono);
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--fg-3);
  }
  .group-h .g-name { color: var(--fg-2); }
  .group-h .g-count {
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    padding: 0 7px;
    border-radius: 999px;
    font-size: 9.5px;
  }

  .strips {
    display: flex;
    gap: 0;
    border-bottom: 1px solid var(--border);
    background: #0c0c0c;
  }

  .fleet-head {
    display: grid;
    grid-template-columns: 14px 18px 1.6fr 1.2fr 90px 80px 96px 80px;
    gap: 14px;
    padding: 9px 16px;
    background: #0a0a0a;
    border-bottom: 1px solid var(--border);
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
  }

  .empty {
    padding: 24px 16px;
    color: var(--fg-3);
    font-size: 12px;
    text-align: center;
  }
  .empty.err { color: var(--crash); }
  .empty .link {
    background: transparent;
    border: 0;
    color: var(--link);
    text-decoration: underline;
    font: inherit;
    cursor: pointer;
  }

  .just-shipped {
    padding: 10px 16px;
    border-top: 1px solid var(--border);
    background: #0c0c0c;
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .just-shipped .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-2);
    padding: 3px 9px;
    border-radius: var(--radius-pill);
    background: var(--bg-2);
    border: 1px solid var(--border-2);
  }
  .chip-dot {
    width: 5px;
    height: 5px;
    border-radius: var(--radius-pill);
    background: var(--green);
  }
  .chip-k {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .chip-t {
    max-width: 240px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip-who {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
  }
  .view-all {
    color: var(--fg-2);
    cursor: pointer;
    text-decoration: none;
  }
  .view-all:hover { color: var(--fg); }

  @media (max-width: 1100px) {
    .hero { grid-template-columns: 1fr; }
    .summary { grid-template-columns: repeat(3, 1fr); gap: 10px; }
  }

  /* Tighten the hero text + collapse the summary to two columns on
     tablets — the 3-up arrangement starts losing readability around
     here once the sidebar is also open. */
  @media (max-width: 1100px) {
    /* The FleetRow drops to 6 columns at 1100px, so the desktop
       8-col header would no longer line up — hide it. The first
       FleetRow visually substitutes thanks to the always-on context
       bar + Open button. */
    .fleet-head { display: none; }
  }
  @media (max-width: 920px) {
    .summary { grid-template-columns: repeat(2, 1fr); }
    .hello { font-size: 26px; }
    .scroll { padding: 14px 14px 14px; gap: 12px; }
    .fleet-h {
      flex-wrap: wrap;
      gap: 8px;
    }
    .fleet-h .seg-btn { padding: 4px 8px; }
  }

  /* Phone layout: full-width summary cards, collapse the fleet header
     row to a single "Session · context" line. The full grid is replaced
     by the card layout in FleetRow's own media query so this just
     suppresses the desktop column header and tightens chrome. */
  @media (max-width: 700px) {
    /* Promote the toolbar to a sticky route header — feels like an iOS
       large-title nav after the hero scrolls past. */
    .toolbar {
      position: sticky;
      top: 0;
      z-index: 5;
      background: color-mix(in srgb, var(--bg-chrome) 92%, transparent);
      backdrop-filter: blur(10px);
      -webkit-backdrop-filter: blur(10px);
      gap: 6px;
      padding-left: 12px;
      padding-right: 12px;
    }
    .scroll { padding: 12px 12px 18px; gap: 14px; }

    /* On phone the user lands here to find their sessions, not stats —
       reorder the flex children so the fleet leads, the stuck panel
       sits above when relevant, and the hero/summary fall to the
       bottom as decorative footer cards. CSS `order` reshuffles
       without DOM churn so desktop/tablet keep the original layout. */
    :global(.scroll > .stuck) { order: -2; }
    .fleet { order: -1; border-radius: 12px; }
    .summary { order: 1; grid-template-columns: 1fr; gap: 10px; }
    .hero { order: 2; gap: 12px; padding-top: 4px; }

    /* Compact the hero so it reads as a footer summary on phone. */
    .hello { font-size: 20px; line-height: 1.15; }
    .narrative { gap: 6px; margin-top: 8px; }
    .hero-side { display: none; }
    .fleet-h {
      padding: 12px 14px;
      flex-wrap: wrap;
      gap: 8px;
    }
    .fleet-h .seg {
      flex: 1 1 auto;
      justify-content: space-around;
    }
    .fleet-h .seg-btn {
      flex: 1;
      padding: 8px 6px;
      font-size: 10.5px;
    }
    /* Hide the "Group" segmented control on phone — least-actionable
       affordance, recovers a row of chrome. */
    .fleet-h .seg:last-of-type { display: none; }

    /* .fleet-head is already hidden by the 1100px rule above. */
    .group-h { padding: 10px 14px 6px; }
    .strips { flex-direction: column; }
    .just-shipped {
      padding: 12px 14px;
      gap: 10px;
    }
    .chip-t { max-width: 160px; }
  }

  @media (max-width: 420px) {
    .hello { font-size: 20px; }
    /* Drop the "of" total once space is tight — keep the streaming
       count, which is the actionable bit. */
    .fleet-h .micro:nth-of-type(2) { display: none; }
    /* Spawn button is duplicated by the bottom-nav FAB on phone — hide
       the toolbar copy so the route header reads "Overview · 24h". */
    .toolbar :global(.tb-btn.primary) { display: none; }
  }
</style>
