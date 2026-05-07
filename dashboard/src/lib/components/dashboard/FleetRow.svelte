<script lang="ts">
  import { goto } from '$app/navigation';
  import type { Session } from '$lib/api';
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

  const state = $derived(deriveState(s));
  const ctx = $derived(ctxOf(s));
  const stateColor = $derived(
    state === 'live' ? 'var(--green)' :
    state === 'compact' ? 'var(--cta)' :
    state === 'crash' ? 'var(--crash)' : 'var(--fg-3)'
  );
  const cColor = $derived(ctxColor(ctx));
  const tColor = $derived(toolColor(s.tool));
  const project = $derived(projectOf(s.workdir));
  const uptime  = $derived(fmtUptime(s.uptime_seconds, s.created_at));
  const ago     = $derived(fmtRel(s.last_activity_at));

  function open() { goto(`/sessions/${s.id}`); }
  function openClick(e: MouseEvent) { e.stopPropagation(); open(); }
</script>

<div
  class="row"
  class:live={state === 'live'}
  class:crash={state === 'crash'}
  class:compact={state === 'compact'}
  role="button"
  tabindex="0"
  onclick={open}
  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } }}
>
  <span
    class="dot"
    style:background={stateColor}
    style:box-shadow={state === 'live' ? '0 0 0 3px rgba(25,214,0,0.12)' : 'none'}
  ></span>

  <div class="title">
    <div class="title-row">
      <span class="name">{s.name}</span>
      {#if s.tool}
        <span class="tool">
          <span class="tdot" style:background={tColor}></span>
          {toolShort(s.tool)}{s.model ? `·${s.model}` : ''}
        </span>
      {/if}
    </div>
    <div class="title-sub">
      <span class="task" title={s.workdir}>{project}</span>
      {#if state === 'live'}<span class="up mono">{uptime}</span>{/if}
    </div>
  </div>

  <div class="last">
    <span class="log">{lastLogLine(s)}</span>
    <span class="ago mono">{ago}</span>
  </div>

  <span class="right num">{fmtTokens(s.tokens)}</span>
  <span class="right num">{fmtCost(s.cost)}</span>

  <div class="ctx">
    <div class="bar"><div class="fill" style:width={`${ctx}%`} style:background={cColor}></div></div>
    <span class="pct" style:color={cColor}>{ctx}%</span>
  </div>

  <div class="open">
    <button type="button" onclick={openClick}>Open</button>
  </div>
</div>

<style>
  .row {
    display: grid;
    grid-template-columns: 14px 1.6fr 1.2fr 90px 80px 96px 80px;
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
  .row:hover { background: #161616; }
  .row.live::before    { background: rgba(25,214,0,0.55); }
  .row.crash::before   { background: var(--crash); }
  .row.compact::before { background: var(--cta); }
  .row:focus-visible { outline: 2px solid var(--link); outline-offset: -2px; }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill);
  }
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
    color: var(--fg-3);
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
</style>
