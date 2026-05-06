<script lang="ts">
  import { goto } from '$app/navigation';
  import type { Session } from '$lib/api';
  import {
    deriveState, ctxOf, ctxColor, fmtTokens, fmtCost,
    toolShort, lastLogLine
  } from '$lib/dashboard';

  /**
   * One row in the fleet table — clickable to V1b session detail.
   * Density target: ~36px tall.
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

  function open() { goto(`/sessions/${s.id}`); }
  function openClick(e: MouseEvent) { e.stopPropagation(); open(); }
</script>

<div
  class="row"
  class:live={state === 'live'}
  role="button"
  tabindex="0"
  onclick={open}
  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(); } }}
>
  <span class="dot" style:background={stateColor} style:box-shadow={state === 'live' ? '0 0 0 3px rgba(25,214,0,0.12)' : 'none'}></span>

  <div class="title">
    <span class="name">{s.name}</span>
    {#if s.tool || s.model}
      <span class="meta">{toolShort(s.tool)}{s.model ? `·${s.model}` : ''}</span>
    {/if}
    <span class="task">{s.workdir}</span>
  </div>

  <div class="last">{lastLogLine(s)}</div>

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
    padding: 11px 16px;
    border-bottom: 1px solid var(--border);
    align-items: center;
    cursor: pointer;
    transition: background var(--t-hover), border-color var(--t-hover);
  }
  .row:hover { background: #161616; border-color: #4a4a4a; }
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
    align-items: baseline;
    gap: 10px;
  }
  .name {
    color: var(--fg);
    font-size: 13.5px;
    letter-spacing: -0.005em;
    white-space: nowrap;
  }
  .meta {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    white-space: nowrap;
  }
  .task {
    font-size: 12px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .last {
    min-width: 0;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--fg-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    transition: color var(--t-hover), border-color var(--t-hover);
  }
  .open button:hover { color: var(--fg); border-color: var(--fg-3); }
</style>
