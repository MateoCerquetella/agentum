import { writable, type Writable, get } from 'svelte/store';
import { tweaks } from './tweaks';
import { notify, requestPermission, notifyPermission } from './notify';
import { playChime } from './chime';
import { applyServerPrefs } from './theme-bridge';
import { profiles, activeProfileId } from '../profiles';
import { wsUrlForProfile } from '../profiles';

export interface BusEvent {
  kind: string;
  session_id: string | null;
  session_name: string | null;
  payload: Record<string, unknown>;
  ts: string;
  /**
   * Source profile id — stamped by the bus before fanout so listeners
   * can attribute the event to a specific endpoint. Undefined for
   * locally-synthesised events.
   */
  profile?: string;
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

/**
 * Live WS connection state for the **active** profile, surfaced for
 * the topbar offline banner. Non-active profile buses run in the
 * background and don't affect this — the banner is the user's primary
 * "is my dashboard live" signal and that's anchored to whichever
 * endpoint they're sitting on.
 */
export type ConnStatus =
  | { state: 'idle' }
  | { state: 'connected' }
  | { state: 'reconnecting'; attempt: number; nextDelayMs: number };

export const connStatus: Writable<ConnStatus> = writable({ state: 'idle' });

// Track HTTP failures alongside the active-profile WS one. The WS
// connect/disconnect flips connStatus directly; the HTTP path can
// flip to `reconnecting` when fetches start failing (daemon went
// away, LAN dropped, …) even before the WS notices via its own
// onclose. Only the WS onopen is allowed to flip back to `connected`
// — that's the authoritative signal.
let httpFailures = 0;
const HTTP_FAIL_THRESHOLD = 2;

export function markFetchOk(): void {
  httpFailures = 0;
}

export function markFetchFailed(): void {
  httpFailures += 1;
  if (httpFailures < HTTP_FAIL_THRESHOLD) return;
  connStatus.update((s) => {
    if (s.state === 'reconnecting') return s;
    return { state: 'reconnecting', attempt: httpFailures, nextDelayMs: 0 };
  });
}

function pushToast(t: Omit<Toast, 'id' | 'created_at'>) {
  const toast: Toast = { ...t, id: nextToastId++, created_at: Date.now() };
  toasts.update((xs) => [...xs, toast]);
  setTimeout(() => {
    toasts.update((xs) => xs.filter((x) => x.id !== toast.id));
  }, t.ttl_ms);
}

export function showToast(t: Omit<Toast, 'id' | 'created_at'>): void {
  pushToast(t);
}

export function dismissToast(id: number) {
  toasts.update((xs) => xs.filter((x) => x.id !== id));
}

// ---------- multi-profile connection registry ----------

interface Conn {
  profileId: string;
  ws: WebSocket | null;
  /** True after `disconnect()` / `closeConn()` — suppresses auto-reconnect. */
  stopRequested: boolean;
  reconnectAttempt: number;
  connectedAt: number;
}

const conns = new Map<string, Conn>();
let started = false;

const RECONNECT_QUIET_MS = 3000;
const FINISHED_DEBOUNCE_MS = 800;

const pendingFinished = new Map<string, ReturnType<typeof setTimeout>>();
function clearPendingFinished(sid: string) {
  const t = pendingFinished.get(sid);
  if (t) {
    clearTimeout(t);
    pendingFinished.delete(sid);
  }
}

function isActive(profileId: string): boolean {
  return get(activeProfileId) === profileId;
}

function eventsUrlFor(profileId: string): string {
  return wsUrlForProfile(profileId, '/api/events');
}

function bind(conn: Conn, socket: WebSocket) {
  socket.onopen = () => {
    conn.reconnectAttempt = 0;
    conn.connectedAt = Date.now();
    if (isActive(conn.profileId)) {
      connStatus.set({ state: 'connected' });
    }
    for (const t of pendingFinished.values()) clearTimeout(t);
    pendingFinished.clear();
  };
  socket.onmessage = (ev) => {
    if (typeof ev.data !== 'string') return;
    let data: BusEvent;
    try {
      data = JSON.parse(ev.data) as BusEvent;
    } catch {
      return;
    }
    data.profile = conn.profileId;
    handle(conn, data);
  };
  socket.onclose = () => {
    if (conn.stopRequested) return;
    const delay = Math.min(1000 * 2 ** conn.reconnectAttempt, 8000);
    conn.reconnectAttempt += 1;
    if (isActive(conn.profileId)) {
      connStatus.set({
        state: 'reconnecting',
        attempt: conn.reconnectAttempt,
        nextDelayMs: delay,
      });
    }
    setTimeout(() => {
      if (!conn.stopRequested) openConn(conn.profileId);
    }, delay);
  };
  socket.onerror = () => {
    socket.close();
  };
}

function openConn(profileId: string) {
  const list = get(profiles);
  const profile = list.find((p) => p.id === profileId);
  if (!profile) return;
  // Skip profiles with no token — the daemon's WS upgrade would 401
  // (auth middleware rejects the bearer-less upgrade). The reconciler
  // will retry once a token lands.
  if (!profile.token) return;

  let conn = conns.get(profileId);
  if (!conn) {
    conn = {
      profileId,
      ws: null,
      stopRequested: false,
      reconnectAttempt: 0,
      connectedAt: 0,
    };
    conns.set(profileId, conn);
  }
  if (conn.ws && conn.ws.readyState <= 1) return;
  conn.stopRequested = false;
  conn.ws = new WebSocket(eventsUrlFor(profileId));
  bind(conn, conn.ws);
}

function closeConn(profileId: string) {
  const conn = conns.get(profileId);
  if (!conn) return;
  conn.stopRequested = true;
  conn.ws?.close();
  conn.ws = null;
  conns.delete(profileId);
}

/**
 * Sync the open WS set against the current profile list. Idempotent —
 * safe to call from a store subscription. Opens new connections,
 * closes connections to profiles that vanished or lost their token.
 */
function reconcile() {
  const list = get(profiles);
  const wanted = new Set(list.filter((p) => !!p.token).map((p) => p.id));
  for (const id of wanted) {
    if (!conns.has(id)) openConn(id);
  }
  for (const [id] of conns) {
    if (!wanted.has(id)) closeConn(id);
  }
  const activeId = get(activeProfileId);
  if (!conns.has(activeId)) {
    connStatus.set({ state: 'idle' });
  }
}

// ---------- fanout to in-tab subscribers ----------

type Listener = (ev: BusEvent) => void;
const listeners: Set<Listener> = new Set();

export function onEvent(cb: Listener): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

function fanOut(ev: BusEvent) {
  for (const cb of listeners) {
    try {
      cb(ev);
    } catch (e) {
      console.error('event listener failed:', e);
    }
  }
}

// ---------- event handler (toast + chime + OS notify) ----------

function handle(conn: Conn, ev: BusEvent) {
  fanOut(ev);
  // Quiet window after (re)connect, scoped per profile: the bus often
  // replays a flurry of watchdog re-classifications when the daemon
  // restarts or the network blips. Let data stores update via fan-out,
  // but suppress human-facing toasts / OS notifications.
  const inQuiet = Date.now() - conn.connectedAt < RECONNECT_QUIET_MS;

  if (ev.kind === 'agent.working' && ev.session_id) {
    clearPendingFinished(ev.session_id);
  }

  switch (ev.kind) {
    case 'watchdog.compact': {
      if (inQuiet || !get(tweaks).notifyCompact) break;
      const name = labelFor(ev);
      pushToast({
        kind: 'info',
        title: `auto-compacted ${name}`,
        body: 'watchdog detected low context and sent /compact',
        ttl_ms: 6000,
      });
      maybeNotify({
        title: `auto-compacted ${name}`,
        body: 'watchdog detected low context and sent /compact',
        tag: `${ev.profile ?? 'global'}.${ev.session_id ?? 'global'}.compact`,
        sessionId: ev.session_id,
      });
      break;
    }
    case 'session.crashed': {
      if (inQuiet || !get(tweaks).notifyCrashed) break;
      if (ev.session_id) clearPendingFinished(ev.session_id);
      const name = labelFor(ev);
      const reason =
        (ev.payload?.reason as string) ?? (ev.payload?.signature as string) ?? 'unknown';
      pushToast({
        kind: 'error',
        title: `${name} crashed`,
        body: `reason: ${reason}`,
        ttl_ms: 12000,
      });
      maybeNotify({
        title: `${name} crashed`,
        body: `reason: ${reason}`,
        tag: `${ev.profile ?? 'global'}.${ev.session_id ?? 'global'}.crashed`,
        sessionId: ev.session_id,
        urgent: true,
      });
      break;
    }
    case 'session.started': {
      break;
    }
    case 'session.stopped': {
      const name = labelFor(ev);
      pushToast({ kind: 'info', title: `${name} stopped`, ttl_ms: 4000 });
      break;
    }
    case 'preferences.changed': {
      // Only adopt theme prefs from the ACTIVE profile — pulling them
      // from background profiles would let a remote daemon overwrite
      // the user's local theme choice every time it broadcasts.
      if (!isActive(conn.profileId)) break;
      applyServerPrefs({
        theme: (ev.payload?.theme as string | null) ?? undefined,
        tui_theme: (ev.payload?.tui_theme as string | null) ?? undefined,
      });
      break;
    }
    case 'bus.lagged': {
      const skipped = ev.payload?.skipped ?? '?';
      pushToast({
        kind: 'warn',
        title: `event stream lagged`,
        body: `${skipped} events skipped`,
        ttl_ms: 4000,
      });
      break;
    }
    case 'board.created': {
      if (inQuiet) break;
      const key = (ev.payload?.key as string) ?? 'ticket';
      const title = (ev.payload?.title as string) ?? '';
      pushToast({ kind: 'info', title: `${key} created`, body: title, ttl_ms: 3500 });
      break;
    }
    case 'board.updated': {
      if (inQuiet) break;
      const key = (ev.payload?.key as string) ?? 'ticket';
      const status = (ev.payload?.status as string) ?? '';
      pushToast({ kind: 'info', title: `${key} → ${status}`, ttl_ms: 2800 });
      break;
    }
    case 'board.claimed': {
      if (inQuiet) break;
      const key = (ev.payload?.key as string) ?? 'ticket';
      const by = (ev.payload?.claimed_by as string) ?? 'someone';
      pushToast({ kind: 'info', title: `${key} claimed`, body: `by ${by}`, ttl_ms: 3000 });
      break;
    }
    case 'board.released': {
      if (inQuiet) break;
      const key = (ev.payload?.key as string) ?? 'ticket';
      pushToast({ kind: 'info', title: `${key} released`, ttl_ms: 2500 });
      break;
    }
    case 'board.deleted': {
      if (inQuiet) break;
      const id = ev.payload?.id ?? '?';
      pushToast({ kind: 'info', title: `ticket ${id} deleted`, ttl_ms: 2500 });
      break;
    }
    case 'agent.finished': {
      if (inQuiet || !get(tweaks).notifyFinished) break;
      const p = ev.payload as { initial?: boolean; replay?: boolean } | undefined;
      if (p?.replay || p?.initial) break;
      const name = labelFor(ev);
      const sid = ev.session_id;
      if (!sid) break;
      clearPendingFinished(sid);
      const t = setTimeout(() => {
        pendingFinished.delete(sid);
        pushToast({ kind: 'info', title: `${name} finished`, ttl_ms: 6000 });
        playChime('finished');
        maybeNotify({
          title: `${name} finished`,
          body: 'agent is back at idle — output ready to review',
          tag: `${ev.profile ?? 'global'}.${sid}.finished`,
          sessionId: sid,
        });
      }, FINISHED_DEBOUNCE_MS);
      pendingFinished.set(sid, t);
      break;
    }
    case 'agent.awaiting_input': {
      if (inQuiet || !get(tweaks).notifyAwaitingInput) break;
      const p = ev.payload as { initial?: boolean; replay?: boolean } | undefined;
      if (p?.initial || p?.replay) break;
      if (ev.session_id) clearPendingFinished(ev.session_id);
      const name = labelFor(ev);
      pushToast({
        kind: 'warn',
        title: `${name} needs input`,
        body: 'agent is waiting on a permission prompt',
        ttl_ms: 8000,
      });
      playChime('attention');
      maybeNotify({
        title: `${name} needs input`,
        body: 'agent is waiting on a permission prompt',
        tag: `${ev.profile ?? 'global'}.${ev.session_id ?? 'global'}.awaiting_input`,
        sessionId: ev.session_id,
        urgent: true,
      });
      break;
    }
  }
}

/**
 * Compose the user-facing session label, suffixing the source profile
 * when the event came from a non-active endpoint so the user can tell
 * "claude finished" on local vs. on a remote VPS apart.
 */
function labelFor(ev: BusEvent): string {
  const base = ev.session_name ?? 'session';
  if (!ev.profile || isActive(ev.profile)) return base;
  const profile = get(profiles).find((p) => p.id === ev.profile);
  const tag = profile?.label || ev.profile;
  return `${base} · @${tag}`;
}

/** Bridge to the OS notify layer. Click-through deep-links to the
 *  session — the session page itself routes its WS through the owning
 *  profile, so cross-profile clicks just navigate. */
function maybeNotify(opts: {
  title: string;
  body?: string;
  tag?: string;
  sessionId?: string | null;
  urgent?: boolean;
}) {
  if (!get(tweaks).notifyBrowser) return;
  if (get(notifyPermission) === 'default') {
    void requestPermission();
    return;
  }
  notify({
    title: opts.title,
    body: opts.body,
    tag: opts.tag,
    urgent: opts.urgent,
    onClick: opts.sessionId
      ? () => {
          if (typeof location !== 'undefined') {
            location.href = `/sessions/${opts.sessionId}`;
          }
        }
      : undefined,
  });
}

// ---------- public lifecycle API ----------

let unsubProfiles: (() => void) | null = null;
let unsubActive: (() => void) | null = null;

/**
 * Start the multi-profile bus. Idempotent. Opens a WebSocket per
 * profile that carries a bearer token; reconciles whenever the
 * profile list (or its tokens) changes. The legacy single-WS contract
 * is preserved via the active-profile binding of `connStatus`.
 */
export function connect() {
  if (started) {
    reconcile();
    return;
  }
  started = true;
  reconcile();
  unsubProfiles = profiles.subscribe(() => reconcile());
  // Active-profile changes are handled by a full page reload in the
  // EndpointSwitcher today, but subscribe defensively so a programmatic
  // switch still updates the banner to track the new active conn.
  unsubActive = activeProfileId.subscribe((id) => {
    const c = conns.get(id);
    if (!c || !c.ws || c.ws.readyState !== WebSocket.OPEN) {
      connStatus.set({ state: 'idle' });
    } else {
      connStatus.set({ state: 'connected' });
    }
  });
}

/** Close every per-profile WS and reset state. */
export function disconnect() {
  started = false;
  unsubProfiles?.();
  unsubActive?.();
  unsubProfiles = null;
  unsubActive = null;
  for (const id of Array.from(conns.keys())) closeConn(id);
  connStatus.set({ state: 'idle' });
}
