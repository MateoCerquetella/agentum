<script lang="ts">
  import { goto } from '$app/navigation';
  import { api, type Session } from '$lib/api';
  import { loadSessions } from '$stores/sessions';
  import { awaitingInput, idleSessions } from '$stores/attention';
  import {
    deriveState, ctxOf, ctxColor, fmtTokens, fmtCost, fmtRel, fmtUptime,
    toolShort, toolColor, projectOf, lastLogLine
  } from '$lib/dashboard';

  /**
   * One row in the fleet table — clickable to V1b session detail.
   * Density target: ~42px tall (two lines of metadata under the name).
   */
  interface Props { s: Session }
  let { s }: Props = $props();

  let pinning = $state(false);
  async function togglePin(e: MouseEvent) {
    e.stopPropagation();
    if (pinning) return;
    pinning = true;
    try {
      await api.patchSession(s.id, { pinned: !s.pinned });
      await loadSessions();
    } catch (err) {
      console.error('pin toggle failed', err);
    } finally {
      pinning = false;
    }
  }

  // Inline start for stopped/crashed sessions — saves the user a trip
  // to the detail page just to kick a dormant terminal back to life.
  let starting = $state(false);
  const isStopped = $derived(s.status === 'stopped' || s.status === 'crashed');
  async function startSession(e: MouseEvent) {
    e.stopPropagation();
    if (starting) return;
    starting = true;
    try {
      await api.startSession(s.id);
      await loadSessions();
    } catch (err) {
      console.error('start session failed', err);
    } finally {
      starting = false;
    }
  }

  const ctx = $derived(ctxOf(s));
  // Lifecycle reflects the live agent-activity overlay: a `running`
  // session whose agent is sitting at the prompt downgrades from
  // `live` to `idle` so the dot mutes instead of staying a misleading
  // pulsing green. An open permission prompt promotes to `attention`.
  // Mirrors the Sidebar's stateClass priority chain.
  const lifecycle = $derived.by(() => {
    const base = deriveState(s);
    if (base === 'crash') return base;
    if ($awaitingInput.has(s.id)) return 'attention';
    if ($idleSessions.has(s.id) && base === 'live') return 'idle';
    return base;
  });
  const stateColor = $derived(
    lifecycle === 'live' ? 'var(--green)' :
    lifecycle === 'compact' ? 'var(--cta)' :
    lifecycle === 'crash' ? 'var(--crash)' :
    lifecycle === 'attention' ? 'var(--amber, #ffb454)' : 'var(--fg-3)'
  );
  const cColor = $derived(ctxColor(ctx));
  const tColor = $derived(toolColor(s.tool));
  const project = $derived(projectOf(s.workdir));
  const uptime  = $derived(fmtUptime(s.uptime_seconds, s.created_at));
  const ago     = $derived(fmtRel(s.last_activity_at));

  // Stale-tinted activity color. Anything past 30 min reads red, past
  // 5 min reads amber, fresh reads neutral. Useful at a glance for
  // spotting "this agent has been idle for ages" vs "just now."
  const staleMin = $derived.by(() => {
    if (!s.last_activity_at) return Infinity;
    const ms = Date.now() - new Date(s.last_activity_at).getTime();
    return Number.isFinite(ms) && ms >= 0 ? ms / 60_000 : Infinity;
  });
  const agoColor = $derived(
    staleMin >= 30 ? 'var(--cta)'
    : staleMin >= 5 ? 'var(--amber)'
    : 'var(--fg-3)'
  );

  function open() { goto(`/sessions/${s.id}`); }
  function openClick(e: MouseEvent) { e.stopPropagation(); open(); }
</script>

<div
  class="row"
  class:live={lifecycle === 'live'}
  class:crash={lifecycle === 'crash'}
  class:compact={lifecycle === 'compact'}
  role="button"
  tabindex="0"
  onclick={open}
  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } }}
>
  <span
    class="dot"
    style:background={stateColor}
    style:box-shadow={lifecycle === 'live' ? '0 0 0 3px rgba(25,214,0,0.12)' : 'none'}
  ></span>

  <button
    type="button"
    class="pin"
    class:on={s.pinned}
    aria-label={s.pinned ? 'Unpin session' : 'Pin session'}
    title={s.pinned ? 'Unpin session' : 'Pin session'}
    onclick={togglePin}
    disabled={pinning}
  >
    {s.pinned ? '★' : '☆'}
  </button>

  <div class="title">
    <div class="title-row">
      <span class="name">{s.name}</span>
      {#if s.tool}
        <span class="tool">
          <span class="tdot" style:background={tColor}></span>
          {toolShort(s.tool)}{s.model ? `·${s.model}` : ''}
        </span>
      {/if}
      {#if s.profile_label}
        <!-- Endpoint pill: which agentum daemon this session lives on.
             Only shown when the row was tagged by the multi-endpoint
             aggregator; single-endpoint runs leave it off so the row
             stays uncluttered. -->
        <span class="endpoint" title={`Endpoint: ${s.profile_label}`}>
          @{s.profile_label}
        </span>
      {/if}
    </div>
    <div class="title-sub">
      <span class="task" title={s.workdir}>{project}</span>
      {#if lifecycle === 'live'}<span class="up mono">{uptime}</span>{/if}
    </div>
  </div>

  <div class="last">
    <span class="log">{lastLogLine(s)}</span>
    <span class="ago mono" style:color={agoColor}>{ago}</span>
  </div>

  <span class="right num">{fmtTokens(s.tokens)}</span>
  <span class="right num">{fmtCost(s.cost)}</span>

  <div class="ctx">
    <div class="bar"><div class="fill" style:width={`${ctx}%`} style:background={cColor}></div></div>
    <span class="pct" style:color={cColor}>{ctx}%</span>
  </div>

  <div class="open">
    {#if isStopped}
      <button
        type="button"
        class="start"
        onclick={startSession}
        disabled={starting}
        title="Start session"
      >
        {starting ? '…' : 'Start'}
      </button>
    {:else}
      <button type="button" onclick={openClick}>Open</button>
    {/if}
  </div>
</div>

<style>
  .row {
    display: grid;
    grid-template-columns: 14px 18px 1.6fr 1.2fr 90px 80px 96px 80px;
    gap: 14px;
    padding: 9px 16px;
    border-bottom: 1px solid var(--border);
    align-items: center;
    cursor: pointer;
    position: relative;
    transition: background var(--t-hover), border-color var(--t-hover);
  }
  .row::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 2px;
    background: transparent;
    transition: background var(--t-hover);
  }
  .row:hover { background: var(--bg-row-hover); }
  .row.live::before    { background: rgba(25,214,0,0.55); }
  .row.crash::before   { background: var(--crash); }
  .row.compact::before { background: var(--cta); }
  .row:focus-visible { outline: 2px solid var(--link); outline-offset: -2px; }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill);
  }
  .pin {
    width: 18px;
    height: 18px;
    padding: 0;
    background: transparent;
    border: 0;
    color: var(--fg-3);
    font-size: 14px;
    line-height: 1;
    cursor: pointer;
    transition: color var(--t-hover), transform var(--t-hover);
  }
  .pin:hover { color: var(--amber); transform: scale(1.15); }
  .pin.on { color: var(--amber); }
  .pin:disabled { cursor: wait; opacity: 0.5; }
  .row.live .dot { animation: fleet-pulse 1.6s infinite; }
  @keyframes fleet-pulse { 50% { opacity: 0.45; } }

  .title {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .title-row {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }
  .name {
    color: var(--fg);
    font-size: 13.5px;
    letter-spacing: -0.005em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tool {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    white-space: nowrap;
  }
  .tdot {
    width: 5px;
    height: 5px;
    border-radius: var(--radius-pill);
  }
  .endpoint {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    padding: 1px 6px;
    background: color-mix(in srgb, var(--cta) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--cta) 35%, var(--border));
    border-radius: var(--radius-pill);
    font-family: var(--mono);
    font-size: 10px;
    color: var(--cta);
    letter-spacing: 0;
    white-space: nowrap;
  }

  .title-sub {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
    color: var(--fg-3);
    font-size: 11.5px;
  }
  .task {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .up {
    font-size: 10px;
    color: var(--fg-3);
    flex-shrink: 0;
  }

  .last {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .log {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ago {
    font-size: 10px;
    transition: color var(--t-hover);
  }

  .num {
    text-align: right;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--fg-2);
  }
  .right { justify-self: end; }

  .ctx {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }
  .ctx .bar {
    width: 50px;
    height: 3px;
    background: var(--bg-2);
    border-radius: var(--radius-pill);
    overflow: hidden;
  }
  .ctx .fill {
    height: 100%;
    border-radius: var(--radius-pill);
    transition: width 220ms ease, background var(--t-hover);
  }
  .ctx .pct {
    font-family: var(--mono);
    font-size: 11.5px;
    min-width: 32px;
    text-align: right;
  }

  .open { display: flex; justify-content: flex-end; }
  .open button {
    padding: 4px 10px;
    border-radius: 4px;
    background: transparent;
    border: 1px solid var(--border-2);
    color: var(--fg-2);
    font-size: 10.5px;
    font-family: var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
    transition: color var(--t-hover), border-color var(--t-hover), background var(--t-hover);
  }
  .open button:hover {
    color: var(--fg);
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 12%, transparent);
  }
  .open button.start {
    color: var(--green);
    border-color: color-mix(in srgb, var(--green) 45%, transparent);
  }
  .open button.start:hover:not(:disabled) {
    color: var(--bg);
    background: var(--green);
    border-color: var(--green);
  }
  .open button.start:disabled { opacity: 0.6; cursor: wait; }

  /* Tablet: drop tokens/cost numerics — they're in the summary cards
     already and the per-row figures aren't actionable from here. The
     activity log stays so users still see "what is this agent doing
     right now". */
  @media (max-width: 1100px) {
    .row {
      grid-template-columns: 14px 18px 1.6fr 1.2fr 96px 80px;
      gap: 12px;
    }
    .num { display: none; }
  }

  /* Narrow tablet: also drop the activity log — anchor the row on
     name, context, and the open button. */
  @media (max-width: 880px) {
    .row {
      grid-template-columns: 14px 18px 1.6fr 96px 80px;
      gap: 10px;
    }
    .last { display: none; }
  }

  /* Phone: collapse the entire row into a stacked card. The dot, pin
     and Open button stay visible at the edges; everything else flows
     into a two-line block. */
  @media (max-width: 700px) {
    .row {
      grid-template-columns: 14px 26px minmax(0, 1fr) auto;
      grid-template-areas:
        "dot pin title open"
        "dot pin meta  meta";
      gap: 6px 10px;
      padding: 14px 14px;
      align-items: start;
    }
    .row:active { background: var(--bg-row-hover); }
    .dot { grid-area: dot; align-self: center; width: 8px; height: 8px; }
    .pin { grid-area: pin; align-self: center; width: 26px; height: 26px; font-size: 16px; }
    .title { grid-area: title; }
    .open { grid-area: open; align-self: center; }

    .last { display: none; }
    .num  { display: none; }

    /* Move ctx into the second row alongside the project label so the
       user always sees how full the agent's context is. */
    .ctx {
      grid-area: meta;
      justify-content: flex-start;
      gap: 10px;
      margin-top: 4px;
      font-size: 11.5px;
      color: var(--fg-3);
    }
    .ctx .bar { width: 80px; height: 4px; }
    .ctx .pct { min-width: 0; }

    .title-row { gap: 8px; }
    .name { font-size: 15px; letter-spacing: -0.005em; }
    .title-sub { font-size: 12px; margin-top: 2px; }

    .open button {
      padding: 8px 12px;
      font-size: 11px;
      min-height: 32px;
    }
  }

  @media (max-width: 420px) {
    .row { padding: 12px 12px; gap: 4px 8px; }
    .tool { display: none; }
  }
</style>
