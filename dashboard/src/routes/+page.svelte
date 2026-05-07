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
  import StatTile from '$components/dashboard/StatTile.svelte';
  import NarrativeRow from '$components/dashboard/NarrativeRow.svelte';
  import FleetRow from '$components/dashboard/FleetRow.svelte';
  import AttentionStrip from '$components/dashboard/AttentionStrip.svelte';

  let username = $state('there');

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
    <span class="pill live">auto-refresh 5s</span>
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

      <div class="hero-stats">
        <StatTile k="Live" v={String(live.length)} sub={`/ ${$sessions.items.length} pane${$sessions.items.length === 1 ? '' : 's'}`} />
        <StatTile
          k="Avg ctx"
          v={`${avgCtx}%`}
          sub="across live sessions"
          accent={avgCtx >= 70 ? 'var(--green)' : avgCtx >= 50 ? 'var(--amber)' : 'var(--cta)'}
        />
        <StatTile
          k="Incidents"
          v={String(incidents.length)}
          sub={incidents.length > 0 ? incidents.map(i => deriveState(i)).join(' · ') : 'none'}
          accent={incidents.length > 0 ? 'var(--amber)' : 'var(--green)'}
        />
        <StatTile k="Tokens 24h" v={fmtTokens(tokens24)} sub="all tools" />
        <StatTile k="Spend 24h" v={fmtCost(spend24)} sub={doneItems.length > 0 ? `${fmtCost(spend24 / doneItems.length)} / ticket` : '—'} />
        <StatTile
          k="Shipped"
          v={String(doneItems.length)}
          sub={`${reviewItems.length} in review`}
          accent="var(--green)"
        />
      </div>
    </section>

    <section class="fleet">
      <div class="fleet-h">
        <span class="micro" style="color: var(--fg);">Fleet</span>
        <span class="micro">· {$sessions.items.length} session{$sessions.items.length === 1 ? '' : 's'} · {live.length} streaming</span>
        <span class="spacer"></span>
        <span class="micro action" role="button" tabindex="0">sort: ctx ↑</span>
        <span class="micro action" role="button" tabindex="0">filter</span>
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
      {:else}
        {#each $sessions.items as s (s.id)}
          <FleetRow {s} />
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
  .hero-stats {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    grid-template-rows: 1fr 1fr;
    gap: 8px;
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
  .fleet-h .action { cursor: pointer; }
  .fleet-h .action:hover { color: var(--fg-2); }

  .strips {
    display: flex;
    gap: 0;
    border-bottom: 1px solid var(--border);
    background: #0c0c0c;
  }

  .fleet-head {
    display: grid;
    grid-template-columns: 14px 1.6fr 1.2fr 90px 80px 96px 80px;
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
    .hero-stats { grid-template-columns: 1fr 1fr; grid-template-rows: auto; }
  }
</style>
