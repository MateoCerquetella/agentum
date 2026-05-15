import { writable, type Writable } from 'svelte/store';
import { onEvent } from './events';

/**
 * Ephemeral activity tracking that augments the per-session snapshot
 * from /api/sessions with bus-derived signals the wire shape doesn't
 * carry:
 *
 *  - `awaitingInput`: agent has a permission prompt or multi-choice
 *    menu open and is blocked on the user. Set on
 *    `agent.awaiting_input`, cleared on `agent.finished` /
 *    `agent.input_resolved` / `session.stopped` / `session.crashed`.
 *
 *  - `idleSessions`: agent has finished its turn and is sitting at
 *    the prompt. Without this, the sidebar dot stays a misleading
 *    "live" green long after the agent went quiet — server status
 *    stays `running` between turns. Set on `agent.finished`, cleared
 *    the moment the watchdog sees the busy spinner again
 *    (`agent.working`) or via `agent.input_resolved` with
 *    `state: "working"`. Mirrors the TUI's `app.idle` set.
 *
 * Both live only as long as the tab; we don't persist because the
 * same signals re-fire on reconnect if still relevant (and stale
 * entries clear on the next `session.deleted`).
 */

export const awaitingInput: Writable<Set<string>> = writable(new Set());
export const idleSessions: Writable<Set<string>> = writable(new Set());

function addTo(store: Writable<Set<string>>, id: string): void {
  store.update((s) => {
    if (s.has(id)) return s;
    const next = new Set(s);
    next.add(id);
    return next;
  });
}

function removeFrom(store: Writable<Set<string>>, id: string): void {
  store.update((s) => {
    if (!s.has(id)) return s;
    const next = new Set(s);
    next.delete(id);
    return next;
  });
}

let booted = false;

export function startAttentionBridge(): void {
  if (booted) return;
  booted = true;
  onEvent((ev) => {
    if (!ev.session_id) return;
    const id = ev.session_id;
    switch (ev.kind) {
      case 'agent.awaiting_input':
        addTo(awaitingInput, id);
        // A session waiting on the user isn't "idle at prompt" —
        // it's blocked. Drop the muted dot so the attention overlay
        // (▲ yellow) wins cleanly.
        removeFrom(idleSessions, id);
        break;
      case 'agent.finished':
        // Turn ended. Park as idle (muted dot) and clear any stale
        // attention flag.
        addTo(idleSessions, id);
        removeFrom(awaitingInput, id);
        break;
      case 'agent.working':
        // Watchdog saw the busy spinner again — agent picked up a
        // new turn. Strip both flags so the live dot returns.
        removeFrom(idleSessions, id);
        removeFrom(awaitingInput, id);
        break;
      case 'agent.input_resolved': {
        removeFrom(awaitingInput, id);
        // Payload tells us whether the agent is back at work or
        // sitting at the prompt; older daemons (pre-v0.6.28) omit
        // it, in which case we leave idle alone and let the next
        // finished/working event settle it.
        const state = (ev.payload as { state?: string } | undefined)?.state;
        if (state === 'idle') addTo(idleSessions, id);
        else if (state === 'working') removeFrom(idleSessions, id);
        break;
      }
      case 'session.stopped':
      case 'session.crashed':
      case 'session.deleted':
        removeFrom(awaitingInput, id);
        removeFrom(idleSessions, id);
        break;
    }
  });
}

/** Minutes since `last_activity_at` (or `updated_at`). Returns Infinity
 *  when neither is set — that way "no activity ever" sorts as more
 *  stale than "20 min ago" rather than less. */
export function staleMinutes(s: { last_activity_at?: string | null; updated_at?: string | null }): number {
  const ts = s.last_activity_at ?? s.updated_at ?? null;
  if (!ts) return Infinity;
  const ms = Date.now() - new Date(ts).getTime();
  if (!Number.isFinite(ms) || ms < 0) return 0;
  return ms / 60_000;
}
