import { onEvent, type BusEvent } from './events';
import { loadSessions } from './sessions';
import { loadBoard } from './board';
import { pushWatchdogEvent } from './watchdog';
import type { WatchdogEvent, WatchdogKind } from '$lib/api';

/**
 * Hydrates the redesigned dashboard's stores from the live event bus
 * exposed by /api/events (WS).
 *
 * - `watchdog.*` and `session.*` events convert to WatchdogEvent and
 *   prepend onto the watchdog store. Same shape as the cold-start
 *   GET /api/watchdog list, so renderers don't branch on source.
 * - Session lifecycle events trigger a debounced `loadSessions()` so
 *   the FleetRow / sidebar list reflects current state without polling.
 * - Board moves (kind starts with "board.") trigger `loadBoard()`.
 */

let unsub: (() => void) | null = null;
let sessionRefreshTimer: ReturnType<typeof setTimeout> | null = null;
let boardRefreshTimer: ReturnType<typeof setTimeout> | null = null;

function debouncedSessions() {
  if (sessionRefreshTimer) clearTimeout(sessionRefreshTimer);
  sessionRefreshTimer = setTimeout(() => loadSessions(), 250);
}

function debouncedBoard() {
  if (boardRefreshTimer) clearTimeout(boardRefreshTimer);
  boardRefreshTimer = setTimeout(() => loadBoard(), 250);
}

function project(ev: BusEvent): WatchdogEvent | null {
  let kind: WatchdogKind;
  if (ev.kind.startsWith('watchdog.')) {
    const rest = ev.kind.slice('watchdog.'.length);
    kind = rest === 'compact' ? 'compact'
         : rest === 'crash'   ? 'crash'
         : rest === 'warn'    ? 'warn'
         : 'ok';
  } else if (ev.kind === 'session.crashed') {
    kind = 'crash';
  } else if (ev.kind.startsWith('session.')) {
    kind = 'ok';
  } else {
    return null;
  }
  const label = (ev.payload?.label as string | undefined) ?? ev.kind.split('.').pop() ?? 'event';
  const msg   = (ev.payload?.msg   as string | undefined)
              ?? (ev.session_name
                    ? `${ev.session_name} · ${ev.kind}`
                    : ev.kind);
  return {
    ts: ev.ts,
    kind,
    label,
    msg,
    ses: ev.session_name
  };
}

export function startEventBridge(): void {
  if (unsub) return;
  unsub = onEvent((ev) => {
    const wd = project(ev);
    if (wd) pushWatchdogEvent(wd);

    if (ev.kind.startsWith('session.') || ev.kind === 'agent.finished') {
      debouncedSessions();
    }
    if (ev.kind.startsWith('board.')) {
      debouncedBoard();
    }
  });
}

export function stopEventBridge(): void {
  unsub?.();
  unsub = null;
  if (sessionRefreshTimer) { clearTimeout(sessionRefreshTimer); sessionRefreshTimer = null; }
  if (boardRefreshTimer)   { clearTimeout(boardRefreshTimer);   boardRefreshTimer = null; }
}
