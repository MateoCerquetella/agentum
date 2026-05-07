import { writable, type Writable } from 'svelte/store';
import { onEvent } from './events';

/**
 * Ephemeral "needs-attention" tracking that augments the per-session
 * snapshot from /api/sessions with bus-derived signals the wire shape
 * doesn't carry — chiefly `agent.awaiting_input`, which is a moment-in-
 * time event, not a persistent boolean on the Session model.
 *
 * Cleared on `agent.finished`, `session.stopped`, or user input — any
 * of which mean the prompt has been answered (or the session is no
 * longer alive). Lives only as long as the tab; we don't persist
 * because the same signal will fire again on reconnect if still pending.
 */

export const awaitingInput: Writable<Set<string>> = writable(new Set());

let booted = false;

export function startAttentionBridge(): void {
  if (booted) return;
  booted = true;
  onEvent((ev) => {
    if (!ev.session_id) return;
    if (ev.kind === 'agent.awaiting_input') {
      awaitingInput.update((s) => {
        if (s.has(ev.session_id!)) return s;
        const next = new Set(s);
        next.add(ev.session_id!);
        return next;
      });
    } else if (
      ev.kind === 'agent.finished'
      || ev.kind === 'session.stopped'
      || ev.kind === 'session.crashed'
    ) {
      awaitingInput.update((s) => {
        if (!s.has(ev.session_id!)) return s;
        const next = new Set(s);
        next.delete(ev.session_id!);
        return next;
      });
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
