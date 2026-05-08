/**
 * Thin fetch wrapper for the agentum HTTP API.
 *
 * Bearer token plumbing is in place but unused in phase 3 (auth middleware
 * lands phase 5). When the token slot is empty, requests go out unauth'd
 * and the backend allows them.
 */

export type Status = 'idle' | 'running' | 'stopped' | 'crashed';

/**
 * Lifecycle dot rendered by the design. `compact` is server-pushed when
 * the watchdog issues /compact; `crash` mirrors `Status.crashed`.
 */
export type SessionState = 'live' | 'idle' | 'compact' | 'crash';

export interface Session {
  id: string;
  name: string;
  workdir: string;
  tool: string;
  model: string | null;
  flags: string[];
  status: Status;
  tmux_target: string | null;
  created_at: string;
  updated_at: string;
  last_activity_at: string | null;
  /* --- Optional fields backfilled by the redesign backend. ----------
     Until the Rust side populates them they're undefined; consumers
     fall back to N/A or 100 (full ctx). */
  /** Tokens consumed by this session. */
  tokens?: number | null;
  /** Spend in USD for this session. */
  cost?: number | null;
  /** 0–100, % of context window remaining. */
  ctx?: number | null;
  /** Tail of stdout — the FleetRow's "last activity" cell. */
  last_log?: string | null;
  /** Uptime in seconds, computed by the server. */
  uptime_seconds?: number | null;
  /** Lifecycle state with /compact awareness; `status` is still authoritative. */
  state?: SessionState;
  /** User-toggled "favorite" — sorts to the top of every list. */
  pinned?: boolean;
  /* --- Multi-endpoint aggregation. Tagged client-side by the
     sessions store; not on the wire. The id matches a `Profile.id`
     in `lib/profiles.ts`. Empty / undefined for sessions returned
     from the active profile or for older single-endpoint flows. */
  /** Owning endpoint profile id. Used by FleetRow's pill + the
   *  per-session terminal page to build the right WS URL. */
  profile?: string;
  /** Display label for the owning profile, captured at fetch time
   *  so renderers don't have to re-derive from the profile store. */
  profile_label?: string;
}

export interface NewSession {
  name: string;
  workdir: string;
  tool: string;
  model?: string | null;
  flags?: string[];
}

/// Mirrors `agentum_server::routes::agents::AgentInfo`. The dashboard
/// uses this to gate the agent picker on whether the underlying CLI
/// is actually installed on the daemon's PATH.
export interface AgentInfo {
  name: string;
  binary: string;
  available: boolean;
  yolo_flag: string | null;
  path: string | null;
}

export interface DoctorCheck {
  label: string;
  passed: boolean;
  detail: string;
}

export interface DoctorReport {
  ok: boolean;
  failures: number;
  checks: DoctorCheck[];
}

export interface AuthStatus {
  needs_setup: boolean;
  register_open: boolean;
}

export interface CertFingerprint {
  /// Empty when running with --no-tls.
  sha256: string;
  tls: boolean;
}

export interface AuthResp {
  token: string;
  username: string;
}

export interface MeResp {
  username: string;
}

export interface DirEntry {
  name: string;
  path: string;
}

export interface DirListing {
  path: string;
  parent: string | null;
  dirs: DirEntry[];
}

export interface Health {
  status: 'ok';
  version: string;
  uptime_seconds: number;
  sessions_running: number;
  /// Optional in older daemons (pre-v0.6.7) — treat absence as "no
  /// optional features supported" so we don't ship messages the server
  /// will misinterpret as keystrokes (e.g. PTY resize).
  capabilities?: string[];
}

import {
  apiUrl,
  clearActiveToken,
  getActiveProfile,
  setActiveToken,
  wsUrl as profileWsUrl
} from './profiles';

function readToken(): string | null {
  return getActiveProfile().token || null;
}

/// Set or clear the bearer token for the *active* profile. Callers
/// shouldn't need to reach into the profile store directly for the
/// common login / logout flow — this preserves the prior single-token
/// shape.
export function setToken(token: string | null) {
  if (token === null) clearActiveToken();
  else setActiveToken(token);
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  if (!headers.has('content-type') && init.body) {
    headers.set('content-type', 'application/json');
  }
  const token = readToken();
  if (token) headers.set('authorization', `Bearer ${token}`);

  const res = await fetch(apiUrl(path), { ...init, headers });
  if (res.status === 401) {
    // Stale or missing token — clear so the gate can re-prompt.
    setToken(null);
    throw new ApiError(401, 'unauthorized');
  }
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new ApiError(res.status, text || res.statusText);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(`HTTP ${status}: ${message}`);
    this.name = 'ApiError';
  }
}

/// Public probe: does the server accept the current (or absent) token?
/// Hits `/api/health` (no auth) just to confirm reachability, then
/// `/api/sessions` to validate the token.
export async function probeAuth(): Promise<'ok' | 'unauthorized' | 'unreachable'> {
  try {
    await request<Health>('/api/health');
  } catch {
    return 'unreachable';
  }
  try {
    await request<Session[]>('/api/sessions');
    return 'ok';
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) return 'unauthorized';
    return 'unreachable';
  }
}

/** Mirror of the server's `routes::preferences::Preferences` struct.
 *  Both keys are optional — partial PUTs only overwrite the fields
 *  present in the body, so the dashboard can update `theme` without
 *  clobbering whatever `tui_theme` the TUI last saved. */
export interface Preferences {
  theme?: string;
  tui_theme?: string;
}

export interface SendInput {
  text?: string;
  keys?: string;
  append_enter?: boolean;
}

/** Ticket type tag the kanban dot color picks up. */
export type TicketLbl = 'bug' | 'feat' | 'chore' | 'spike';
/** Tool tag mapped to the ticket's tool dot. */
export type Tool = 'claude' | 'codex' | 'gemini' | 'hermes' | string;

export interface BoardItem {
  id: number;
  key: string;
  title: string;
  body: string | null;
  status: string;
  claimed_by: string | null;
  created_at: string;
  updated_at: string;
  /* Optional design fields; backend lands them in the redesign branch. */
  lbl?: TicketLbl | null;
  tool?: Tool | null;
}

export interface NewBoardItem {
  title: string;
  body?: string | null;
  status?: string | null;
  lbl?: TicketLbl | null;
  tool?: Tool | null;
}

export interface BoardPatch {
  title?: string;
  body?: string | null;
  status?: string;
  lbl?: TicketLbl | null;
  tool?: Tool | null;
}

/* ------------------------------------------------------------------- */
/* Watchdog feed — surfaced in the V1 hero, V1b session rail, and V2
   right rail. Server pushes events over SSE; REST GET is the cold-start
   fetch. */

export type WatchdogKind = 'ok' | 'warn' | 'compact' | 'crash';

export interface WatchdogEvent {
  /** ISO 8601 timestamp; UI renders HH:MM:SS in the local zone. */
  ts: string;
  kind: WatchdogKind;
  /** Short uppercase tag — the pill label. */
  label: string;
  /** Human-readable detail. */
  msg: string;
  /** Originating session id (nullable for fleet-level events). */
  ses: string | null;
}

export interface GroupedBoard {
  columns: Record<string, BoardItem[]>;
  column_order: string[];
}

export interface Note {
  id: number;
  title: string;
  body: string;
  tags: string[];
  created_at: string;
  updated_at: string;
}

export interface NewNote {
  title: string;
  body?: string;
  tags?: string[];
}

export interface NotePatch {
  title?: string;
  body?: string;
  tags?: string[];
}

export interface Channel {
  id: number;
  a_session: string;
  b_session: string;
  created_at: string;
}

export interface NewChannel {
  a_session: string;
  b_session: string;
}

export interface Message {
  id: number;
  channel_id: number;
  sender: string;
  body: string;
  ts: string;
}

export interface NewMessage {
  sender: string;
  body: string;
}

export const api = {
  health: () => request<Health>('/api/health'),
  authStatus: () => request<AuthStatus>('/api/auth/status'),
  login: (username: string, password: string) =>
    request<AuthResp>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password })
    }),
  register: (username: string, password: string) =>
    request<AuthResp>('/api/auth/register', {
      method: 'POST',
      body: JSON.stringify({ username, password })
    }),
  certFingerprint: () => request<CertFingerprint>('/api/cert/fingerprint'),
  logout: () => request<void>('/api/auth/logout', { method: 'POST' }),
  me: () => request<MeResp>('/api/auth/me'),
  listAgents: () => request<AgentInfo[]>('/api/agents'),
  listSessions: (status?: Status) => {
    const qs = status ? `?status=${encodeURIComponent(status)}` : '';
    return request<Session[]>(`/api/sessions${qs}`);
  },
  getSession: (id: string) => request<Session>(`/api/sessions/${encodeURIComponent(id)}`),
  createSession: (body: NewSession) =>
    request<Session>('/api/sessions', { method: 'POST', body: JSON.stringify(body) }),
  startSession: (id: string) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}/start`, { method: 'POST' }),
  stopSession: (id: string) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}/stop`, { method: 'POST' }),
  killSession: (id: string) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}/kill`, { method: 'POST' }),
  patchSession: (
    id: string,
    body: { flags?: string[]; model?: string | null; name?: string; tool?: string; pinned?: boolean }
  ) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}`, { method: 'PATCH', body: JSON.stringify(body) }),
  deleteSession: (id: string, force = false) =>
    request<void>(
      `/api/sessions/${encodeURIComponent(id)}${force ? '?force=true' : ''}`,
      { method: 'DELETE' }
    ),
  doctor: () => request<DoctorReport>('/api/doctor'),

  // ---------- preferences (shared with TUI) ----------

  getPreferences: () => request<Preferences>('/api/preferences'),
  putPreferences: (body: Preferences) =>
    request<Preferences>('/api/preferences', {
      method: 'PUT',
      body: JSON.stringify(body)
    }),

  // ---------- filesystem (workdir picker) ----------

  listDir: (path?: string) => {
    const qs = path ? `?path=${encodeURIComponent(path)}` : '';
    return request<DirListing>(`/api/fs/list${qs}`);
  },
  sendInput: (id: string, body: SendInput) =>
    request<void>(`/api/sessions/${encodeURIComponent(id)}/send`, {
      method: 'POST',
      body: JSON.stringify(body)
    }),

  /**
   * Open a WebSocket to the session's pane stream. Caller is responsible
   * for closing it. Resolves against the active profile's base URL when
   * one is set, otherwise the current page origin (vite proxy in dev,
   * embedded SPA in prod). Browsers cannot set custom headers on WS
   * upgrades, so the bearer token is passed as a `?token=` query param
   * (the backend accepts both forms).
   */
  streamUrl(id: string): string {
    return profileWsUrl(`/api/sessions/${encodeURIComponent(id)}/stream`);
  },

  /// Same wiring for `/api/events` so a profile switch routes the
  /// global event stream at the new server. Components that opened a
  /// stream before the switch are responsible for tearing the old one
  /// down — the URL helper alone can't migrate an in-flight socket.
  eventsUrl(): string {
    return profileWsUrl('/api/events');
  },

  // ---------- board ----------

  listBoard: () => request<GroupedBoard>('/api/board'),
  createBoardItem: (body: NewBoardItem) =>
    request<BoardItem>('/api/board', { method: 'POST', body: JSON.stringify(body) }),
  patchBoardItem: (id: number, body: BoardPatch) =>
    request<BoardItem>(`/api/board/${id}`, { method: 'PATCH', body: JSON.stringify(body) }),
  deleteBoardItem: (id: number) =>
    request<void>(`/api/board/${id}`, { method: 'DELETE' }),
  claimBoardItem: (id: number, claimed_by: string) =>
    request<BoardItem>(`/api/board/${id}/claim`, {
      method: 'POST',
      body: JSON.stringify({ claimed_by })
    }),

  // ---------- watchdog ----------

  /**
   * Cold-start fetch for the watchdog feed. Live updates land via the
   * SSE stream once the server endpoint exists; this is the fallback
   * when SSE is disconnected or the page first loads.
   */
  listWatchdog: (limit?: number) => {
    const q = typeof limit === 'number' ? `?limit=${limit}` : '';
    return request<WatchdogEvent[]>(`/api/watchdog${q}`).catch(
      // Backend lands the endpoint in a follow-up commit; until then
      // return an empty feed instead of erroring.
      () => [] as WatchdogEvent[]
    );
  },

  // ---------- notes ----------

  listNotes: () => request<Note[]>('/api/notes'),
  getNote: (id: number) => request<Note>(`/api/notes/${id}`),
  createNote: (body: NewNote) =>
    request<Note>('/api/notes', { method: 'POST', body: JSON.stringify(body) }),
  patchNote: (id: number, body: NotePatch) =>
    request<Note>(`/api/notes/${id}`, { method: 'PATCH', body: JSON.stringify(body) }),
  deleteNote: (id: number) =>
    request<void>(`/api/notes/${id}`, { method: 'DELETE' }),

  // ---------- channels + messages ----------

  listChannels: () => request<Channel[]>('/api/channels'),
  createChannel: (body: NewChannel) =>
    request<Channel>('/api/channels', { method: 'POST', body: JSON.stringify(body) }),
  deleteChannel: (id: number) =>
    request<void>(`/api/channels/${id}`, { method: 'DELETE' }),
  listMessages: (channelId: number, limit = 200) =>
    request<Message[]>(`/api/channels/${channelId}/messages?limit=${limit}`),
  postMessage: (channelId: number, body: NewMessage) =>
    request<Message>(`/api/channels/${channelId}/messages`, {
      method: 'POST',
      body: JSON.stringify(body)
    })
};
