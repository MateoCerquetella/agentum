import { writable, type Writable, get } from 'svelte/store';
import { tweaks } from './tweaks';
import { notify } from './notify';
import { applyServerPrefs } from './theme-bridge';

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

/**
 * Live WS connection state, surfaced for the topbar offline banner.
 * `idle` = haven't tried to connect yet (page just loaded);
 * `connected` = socket open;
 * `reconnecting` = socket closed, backoff in flight. `attempt` follows
 *   the same counter as the WS handler so a `>= 2` test in the UI
 *   debounces single-blip reconnects.
 */
export type ConnStatus =
  | { state: 'idle' }
  | { state: 'connected' }
  | { state: 'reconnecting'; attempt: number; nextDelayMs: number };

export const connStatus: Writable<ConnStatus> = writable({ state: 'idle' });

// Track HTTP failures alongside the WS one. The WS connect/disconnect
// flips connStatus directly; the HTTP path can flip to `reconnecting`
// when fetches start failing (daemon went away, LAN dropped, …) even
// before the WS notices via its own onclose. Only the WS onopen is
// allowed to flip back to `connected` — that's the authoritative
// signal. Counted as "the next WS retry attempt" so the same `>= 2`
// threshold in the UI keeps both halves debounced consistently.
let httpFailures = 0;
const HTTP_FAIL_THRESHOLD = 2;

/** Called by the HTTP request layer on every successful fetch. */
export function markFetchOk(): void {
  httpFailures = 0;
  // Do NOT flip to `connected` here — the WS is the source of truth
  // for the bus being live. A single successful HTTP probe doesn't
  // mean the event stream is back.
}

/**
 * Called when a fetch throws (network error) or returns 5xx. 4xx is
 * not a network problem so skip; 401 is the gate's job. We count
 * consecutive failures and flip to `reconnecting` once we cross the
 * threshold, mirroring the WS retry counter.
 */
export function markFetchFailed(): void {
  httpFailures += 1;
  if (httpFailures < HTTP_FAIL_THRESHOLD) return;
  connStatus.update((s) => {
    // Already reconnecting via the WS path? Leave it — the WS-side
    // attempt counter is more precise.
    if (s.state === 'reconnecting') return s;
    return {
      state: 'reconnecting',
      attempt: httpFailures,
      nextDelayMs: 0,
    };
  });
}

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

/**
 * Wall-clock time the WS connected. Toasts/notifications fired within
 * `RECONNECT_QUIET_MS` of that moment are suppressed — when the daemon
 * restarts or the network blips, the watchdog often re-classifies a
 * handful of panes simultaneously and we'd otherwise stack a toast per
 * session for events the user did not cause and cannot meaningfully
 * act on.
 */
let connectedAt = 0;
const RECONNECT_QUIET_MS = 3000;

/**
 * Pending `agent.finished` notifications, keyed by session id. We
 * defer them by `FINISHED_DEBOUNCE_MS` so a transient Working→Idle→
 * Working flicker — common when a tool call returns an error and the
 * agent immediately retries — gets cancelled by the follow-up
 * `agent.working` event instead of toasting "X finished" mid-turn.
 */
const FINISHED_DEBOUNCE_MS = 2500;
const pendingFinished = new Map<string, ReturnType<typeof setTimeout>>();
function clearPendingFinished(sid: string) {
  const t = pendingFinished.get(sid);
  if (t) {
    clearTimeout(t);
    pendingFinished.delete(sid);
  }
}

// Wire the events WS through the profile-aware helper so a profile
// switch routes future reconnects at the new endpoint. The legacy
// inline URL builder pre-dated multi-endpoint support and only ever
// looked at the page origin; that broke remote-profile event streams.
import { eventsUrlForActiveProfile } from './profile-bridge';

function eventsUrl(): string {
  return eventsUrlForActiveProfile();
}

function bind(socket: WebSocket) {
  socket.onopen = () => {
    reconnectAttempt = 0;
    connectedAt = Date.now();
    connStatus.set({ state: 'connected' });
    // Drop any pending finished-toasts queued from before the
    // disconnect — the bus state is fresh now and replaying them
    // would lie about current activity.
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
    handle(data);
  };
  socket.onclose = () => {
    if (stopRequested) return;
    // Exponential backoff up to 8s (matches the TUI's events-bus task).
    const delay = Math.min(1000 * 2 ** reconnectAttempt, 8000);
    reconnectAttempt += 1;
    connStatus.set({
      state: 'reconnecting',
      attempt: reconnectAttempt,
      nextDelayMs: delay,
    });
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
  // Quiet window after (re)connect: the bus often replays a flurry of
  // watchdog re-classifications when the daemon restarts or the WS
  // reconnects across a network blip. Let the data stores update via
  // the event-bridge fan-out above, but suppress human-facing toasts /
  // OS notifications so the user doesn't see "X finished" for every
  // running pane.
  const inQuiet = Date.now() - connectedAt < RECONNECT_QUIET_MS;

  // `agent.working` always cancels any pending finished-toast for the
  // same session — even inside the quiet window. A genuine flicker
  // (Working → Idle → Working within ~2 s) shouldn't toast, period.
  if (ev.kind === 'agent.working' && ev.session_id) {
    clearPendingFinished(ev.session_id);
  }

  switch (ev.kind) {
    case 'watchdog.compact': {
      if (inQuiet || !get(tweaks).notifyCompact) break;
      const name = ev.session_name ?? 'session';
      pushToast({
        kind: 'info',
        title: `auto-compacted ${name}`,
        body: 'watchdog detected low context and sent /compact',
        ttl_ms: 6000
      });
      maybeNotify({
        title: `auto-compacted ${name}`,
        body: 'watchdog detected low context and sent /compact',
        tag: `${ev.session_id ?? 'global'}.compact`,
        sessionId: ev.session_id
      });
      break;
    }
    case 'session.crashed': {
      if (inQuiet || !get(tweaks).notifyCrashed) break;
      // A crash invalidates any pending "finished" — never toast both.
      if (ev.session_id) clearPendingFinished(ev.session_id);
      const name = ev.session_name ?? 'session';
      const reason = (ev.payload?.reason as string) ?? (ev.payload?.signature as string) ?? 'unknown';
      pushToast({
        kind: 'error',
        title: `${name} crashed`,
        body: `reason: ${reason}`,
        ttl_ms: 12000
      });
      // Crashes are urgent — fire even if the user is staring at the
      // dashboard. They almost certainly want to triage immediately.
      maybeNotify({
        title: `${name} crashed`,
        body: `reason: ${reason}`,
        tag: `${ev.session_id ?? 'global'}.crashed`,
        sessionId: ev.session_id,
        urgent: true
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
    case 'preferences.changed': {
      // The TUI (or another dashboard tab) just persisted a different
      // theme. Adopt it locally before any user-visible toast — the
      // bridge guards against echo loops via `lastSent`.
      applyServerPrefs({
        theme: (ev.payload?.theme as string | null) ?? undefined,
        tui_theme: (ev.payload?.tui_theme as string | null) ?? undefined
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
      if (inQuiet || !get(tweaks).notifyFinished) break;
      // `initial: true` events fire the first time the watchdog
      // observes a session as idle (daemon restart, first connect).
      // The attention store still picks them up so the dot mutes —
      // but a toast would be stale because the turn ended before the
      // user tuned in.
      if ((ev.payload as { initial?: boolean } | undefined)?.initial) break;
      // Defer the user-facing notification by FINISHED_DEBOUNCE_MS —
      // any `agent.working` event for the same session within that
      // window cancels it, swallowing transient Working→Idle→Working
      // flickers (tool error + retry, brief shell-out, etc.) so the
      // user isn't told the agent finished when it's still mid-turn.
      const name = ev.session_name ?? 'agent';
      const sid = ev.session_id;
      if (!sid) break; // can't debounce without a key — skip
      clearPendingFinished(sid);
      const t = setTimeout(() => {
        pendingFinished.delete(sid);
        // Always toast — matches the TUI's behaviour. A long agent
        // run that finishes silently under a focused-but-not-staring
        // user is exactly the case people are tabbed away from their
        // browser tab for; the pre-v0.7.48 viewing-session
        // suppression meant they saw nothing at all and missed the
        // turn ending.
        pushToast({
          kind: 'info',
          title: `${name} finished`,
          ttl_ms: 6000
        });
        maybeNotify({
          title: `${name} finished`,
          body: 'agent is back at idle — output ready to review',
          tag: `${sid}.finished`,
          sessionId: sid
        });
      }, FINISHED_DEBOUNCE_MS);
      pendingFinished.set(sid, t);
      break;
    }
    case 'agent.awaiting_input': {
      if (inQuiet || !get(tweaks).notifyAwaitingInput) break;
      // `initial: true` means the watchdog tuned in on an
      // already-blocked agent — the attention store still flips
      // the dot, but skip the toast because there's nothing new
      // that demands an immediate user response.
      if ((ev.payload as { initial?: boolean } | undefined)?.initial) break;
      // An open prompt supersedes any pending "finished" for this
      // session — the agent isn't done, it's waiting on you.
      if (ev.session_id) clearPendingFinished(ev.session_id);
      // Permission prompt is open. Always toast — this is a "you have to
      // do something" event and the user might be on another tab. The
      // OS-level notify is urgent so it fires even when the dashboard
      // is foregrounded; missing this one is the worst-case for the
      // user (agent halts until they answer).
      const name = ev.session_name ?? 'agent';
      pushToast({
        kind: 'warn',
        title: `${name} needs input`,
        body: 'agent is waiting on a permission prompt',
        ttl_ms: 8000
      });
      maybeNotify({
        title: `${name} needs input`,
        body: 'agent is waiting on a permission prompt',
        tag: `${ev.session_id ?? 'global'}.awaiting_input`,
        sessionId: ev.session_id,
        urgent: true
      });
      break;
    }
  }
}

/** Bridge to the OS notify layer. Gated on `notifyBrowser` and clicks
 *  jump to the relevant session if one is attached. */
function maybeNotify(opts: {
  title: string;
  body?: string;
  tag?: string;
  sessionId?: string | null;
  urgent?: boolean;
}) {
  if (!get(tweaks).notifyBrowser) return;
  notify({
    title: opts.title,
    body: opts.body,
    tag: opts.tag,
    urgent: opts.urgent,
    onClick: opts.sessionId
      ? () => {
          // Best-effort deep-link. window.focus() already ran inside
          // notify(); follow up by routing to the session so the user
          // lands where they need to act.
          if (typeof location !== 'undefined') {
            location.href = `/sessions/${opts.sessionId}`;
          }
        }
      : undefined
  });
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
  // Caller asked for shutdown; don't leave a stale "connected" status
  // floating in the UI. Hide the banner by reverting to idle.
  connStatus.set({ state: 'idle' });
}
