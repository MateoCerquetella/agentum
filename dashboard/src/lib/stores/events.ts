import { writable, type Writable } from 'svelte/store';

export interface BusEvent {
  kind: string;
  session_id: string | null;
  session_name: string | null;
  payload: Record<string, unknown>;
  ts: string;
}

export interface Toast {
  id: number;
  kind: 'info' | 'warn' | 'error';
  title: string;
  body?: string;
  ttl_ms: number;
  created_at: number;
}

let nextToastId = 1;

export const toasts: Writable<Toast[]> = writable([]);

function pushToast(t: Omit<Toast, 'id' | 'created_at'>) {
  const toast: Toast = { ...t, id: nextToastId++, created_at: Date.now() };
  toasts.update((xs) => [...xs, toast]);
  setTimeout(() => {
    toasts.update((xs) => xs.filter((x) => x.id !== toast.id));
  }, t.ttl_ms);
}

export function dismissToast(id: number) {
  toasts.update((xs) => xs.filter((x) => x.id !== id));
}

let ws: WebSocket | null = null;
let stopRequested = false;
let reconnectAttempt = 0;

function eventsUrl(): string {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = localStorage.getItem('agentum_token') ?? '';
  const qs = token ? `?token=${encodeURIComponent(token)}` : '';
  return `${proto}//${location.host}/api/events${qs}`;
}

function bind(socket: WebSocket) {
  socket.onopen = () => {
    reconnectAttempt = 0;
  };
  socket.onmessage = (ev) => {
    if (typeof ev.data !== 'string') return;
    let data: BusEvent;
    try {
      data = JSON.parse(ev.data) as BusEvent;
    } catch {
      return;
    }
    handle(data);
  };
  socket.onclose = () => {
    if (stopRequested) return;
    // Linear backoff up to 8s.
    const delay = Math.min(1000 * 2 ** reconnectAttempt, 8000);
    reconnectAttempt += 1;
    setTimeout(connect, delay);
  };
  socket.onerror = () => {
    socket.close();
  };
}

type Listener = (ev: BusEvent) => void;
const listeners: Set<Listener> = new Set();

/** Subscribe to every bus event. Returns an unsubscribe callback. */
export function onEvent(cb: Listener): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

function fanOut(ev: BusEvent) {
  for (const cb of listeners) {
    try { cb(ev); } catch (e) { console.error('event listener failed:', e); }
  }
}

function handle(ev: BusEvent) {
  fanOut(ev);
  switch (ev.kind) {
    case 'watchdog.compact': {
      const name = ev.session_name ?? 'session';
      pushToast({
        kind: 'info',
        title: `auto-compacted ${name}`,
        body: 'watchdog detected low context and sent /compact',
        ttl_ms: 6000
      });
      break;
    }
    case 'session.crashed': {
      const name = ev.session_name ?? 'session';
      const reason = (ev.payload?.reason as string) ?? (ev.payload?.signature as string) ?? 'unknown';
      pushToast({
        kind: 'error',
        title: `${name} crashed`,
        body: `reason: ${reason}`,
        ttl_ms: 12000
      });
      break;
    }
    case 'session.started': {
      // No toast — this can be noisy on first connection. Silent for now.
      break;
    }
    case 'session.stopped': {
      const name = ev.session_name ?? 'session';
      pushToast({
        kind: 'info',
        title: `${name} stopped`,
        ttl_ms: 4000
      });
      break;
    }
    case 'bus.lagged': {
      const skipped = ev.payload?.skipped ?? '?';
      pushToast({
        kind: 'warn',
        title: `event stream lagged`,
        body: `${skipped} events skipped`,
        ttl_ms: 4000
      });
      break;
    }
    case 'agent.finished': {
      // Watchdog saw the agent's busy spinner go away. Suppress the
      // toast when the user is already on this session's detail page —
      // they can see "finished" in the pane in front of them.
      const name = ev.session_name ?? 'agent';
      if (!isViewingSession(ev.session_id)) {
        pushToast({
          kind: 'info',
          title: `${name} finished`,
          ttl_ms: 6000
        });
      }
      break;
    }
    case 'agent.awaiting_input': {
      // Permission prompt is open. Always toast — this is a "you have to
      // do something" event and the user might be on another tab.
      const name = ev.session_name ?? 'agent';
      pushToast({
        kind: 'warn',
        title: `${name} needs input`,
        body: 'agent is waiting on a permission prompt',
        ttl_ms: 8000
      });
      break;
    }
  }
}

/** Best-effort "is the user looking at this session right now?" check.
 *  Matches the route `/sessions/{id}` against the current pathname.
 *  Used to suppress redundant `agent.finished` toasts. */
function isViewingSession(id: string | null): boolean {
  if (!id) return false;
  if (typeof location === 'undefined') return false;
  return location.pathname.startsWith(`/sessions/${id}`);
}

export function connect() {
  if (ws && ws.readyState <= 1) return;
  stopRequested = false;
  ws = new WebSocket(eventsUrl());
  bind(ws);
}

export function disconnect() {
  stopRequested = true;
  ws?.close();
  ws = null;
}
