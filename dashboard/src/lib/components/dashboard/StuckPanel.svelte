<script lang="ts">
  /**
   * "What's stuck" — sessions that need a human eye:
   *
   *   1. Awaiting permission prompt (live `agent.awaiting_input` event,
   *      cleared on finish/stop/crash).
   *   2. Idle for > stuckMinutes since last activity, while the pane is
   *      still running. Catches agents that quietly stalled — typically
   *      a tool waiting on remote IO that timed out without crashing.
   *
   * Pure derived view; no extra API calls. Hidden when nothing
   * qualifies so the dashboard doesn't grow a permanent empty box.
   */
  import { goto } from '$app/navigation';
  import type { Session } from '$lib/api';
  import { sessions } from '$stores/sessions';
  import { awaitingInput, staleMinutes } from '$stores/attention';
  import { deriveState } from '$lib/dashboard';
  import { api } from '$lib/api';

  interface Props {
    /** Idle threshold in minutes; sessions stale longer than this surface. */
    stuckMinutes?: number;
  }
  let { stuckMinutes = 5 }: Props = $props();

  type Reason = 'awaiting' | 'idle';
  interface StuckRow {
    s: Session;
    reason: Reason;
    minutes: number;
  }

  const rows: StuckRow[] = $derived.by(() => {
    const items = $sessions.items;
    const out: StuckRow[] = [];
    for (const s of items) {
      if (s.status !== 'running') continue;
      if ($awaitingInput.has(s.id)) {
        out.push({ s, reason: 'awaiting', minutes: staleMinutes(s) });
        continue;
      }
      const state = deriveState(s);
      if (state !== 'idle') continue;
      const m = staleMinutes(s);
      if (m >= stuckMinutes) out.push({ s, reason: 'idle', minutes: m });
    }
    // Awaiting-input always sorts first; within each bucket the oldest
    // wins so the user clears the most-overdue one first.
    out.sort((a, b) => {
      if (a.reason !== b.reason) return a.reason === 'awaiting' ? -1 : 1;
      return b.minutes - a.minutes;
    });
    return out;
  });

  function fmtMin(m: number): string {
    if (!Number.isFinite(m)) return 'never';
    if (m < 1) return 'just now';
    if (m < 60) return `${Math.floor(m)}m`;
    const h = Math.floor(m / 60);
    const rem = Math.floor(m % 60);
    return rem === 0 ? `${h}h` : `${h}h${rem}m`;
  }

  async function unblock(id: string) {
    // Send a single Enter — common case for a "Yes (1)" Claude prompt.
    // For idle stalls, Enter is a no-op which is fine.
    try {
      await api.sendInput(id, { keys: '', append_enter: true });
    } catch (e) { console.error('unblock failed', e); }
  }
</script>

{#if rows.length > 0}
  <section class="stuck">
    <div class="head">
      <span class="dot"></span>
      <span class="title">Needs attention</span>
      <span class="count">{rows.length}</span>
    </div>
    <div class="rows">
      {#each rows as r (r.s.id)}
        <div class="row" class:awaiting={r.reason === 'awaiting'}>
          <span class="reason">{r.reason === 'awaiting' ? 'awaiting input' : `idle ${fmtMin(r.minutes)}`}</span>
          <button type="button" class="name" onclick={() => goto(`/sessions/${r.s.id}`)}>
            {r.s.name}
          </button>
          <span class="meta">{r.s.tool ?? ''}</span>
          <span class="spacer"></span>
          {#if r.reason === 'awaiting'}
            <button type="button" class="act" onclick={() => unblock(r.s.id)}>Send Enter</button>
          {/if}
          <button type="button" class="open" onclick={() => goto(`/sessions/${r.s.id}`)}>Open →</button>
        </div>
      {/each}
    </div>
  </section>
{/if}

<style>
  .stuck {
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-left: 3px solid var(--amber);
    border-radius: var(--radius-lg);
    padding: 10px 14px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .head { display: flex; align-items: center; gap: 8px; }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--amber);
    box-shadow: 0 0 0 3px color-mix(in oklab, var(--amber) 30%, transparent);
  }
  .title {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--fg-2);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .count {
    font-family: var(--mono);
    font-size: 10.5px;
    background: color-mix(in oklab, var(--amber) 18%, transparent);
    color: var(--amber);
    padding: 1px 7px;
    border-radius: 999px;
  }
  .rows { display: flex; flex-direction: column; gap: 4px; }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: var(--radius);
    font-family: var(--mono);
    font-size: 12px;
  }
  .row.awaiting {
    background: color-mix(in oklab, var(--amber) 8%, transparent);
  }
  .reason {
    color: var(--amber);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 10.5px;
    min-width: 110px;
  }
  .row:not(.awaiting) .reason { color: var(--fg-3); }
  .name {
    background: none;
    border: 0;
    padding: 0;
    color: var(--fg);
    cursor: pointer;
    font: inherit;
    text-align: left;
  }
  .name:hover { color: var(--cta); }
  .meta { color: var(--fg-3); font-size: 11px; }
  .spacer { flex: 1; }
  .act, .open {
    background: var(--surface-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius);
    color: var(--fg);
    padding: 3px 9px;
    font: inherit;
    cursor: pointer;
  }
  .act { color: var(--amber); border-color: color-mix(in oklab, var(--amber) 40%, var(--border-2)); }
  .open:hover, .act:hover { border-color: var(--cta); color: var(--cta); }

  /* Phone: the row was a single horizontal flex which crammed the
     reason / name / actions into 360px and clipped. Rework into a
     two-line block: name + reason on top, action buttons full-width
     underneath. */
  @media (max-width: 700px) {
    .stuck { padding: 12px 14px; }
    .row {
      display: grid;
      grid-template-columns: 1fr auto;
      grid-template-areas:
        "name  reason"
        "act   open";
      gap: 6px 10px;
      padding: 10px 8px;
      align-items: center;
    }
    .reason { grid-area: reason; min-width: 0; text-align: right; font-size: 10px; }
    .name   { grid-area: name; font-size: 14px; }
    .meta   { display: none; }
    .spacer { display: none; }
    .act, .open {
      padding: 8px 12px;
      min-height: 36px;
      font-size: 12px;
    }
    .act  { grid-area: act; }
    .open { grid-area: open; }
    /* When there's no .act, span open across both columns. */
    .row:not(.awaiting) .open { grid-column: 1 / span 2; grid-area: auto; }
  }
</style>
