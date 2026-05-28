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
  host_id?: string | null;
  host_label?: string | null;
  host_kind?: string | null;
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
  /** Board card this session is bound to (migration 0011). Absent / null
   *  when the session is not linked to any card. */
  card_id?: number | null;
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
  host_id?: string | null;
  /// Opt-in `git worktree add` request. When present, the server creates
  /// a sibling worktree at `<repo>-worktrees/<branch>` and uses it as
  /// the agent's cwd instead of `workdir`. Both inner fields are
  /// optional: `branch` defaults to a slugified session name prefixed
  /// `agentum/`; `base_ref` defaults to `HEAD`.
  worktree?: { branch?: string; base_ref?: string } | null;
}

export type Host =
  | {
      id: string;
      name: string;
      kind: 'local';
      created_at: string;
      updated_at: string;
      last_seen_at: string | null;
    }
  | {
      id: string;
      name: string;
      kind: 'ssh';
      user: string;
      hostname: string;
      port: number;
      auth: { auth: 'agent' } | { auth: 'key'; path: string };
      created_at: string;
      updated_at: string;
      last_seen_at: string | null;
    };

export type NewHost = {
  name: string;
  kind: 'ssh';
  user: string;
  hostname: string;
  port: number;
  auth?: { auth: 'agent' } | { auth: 'key'; path: string };
};

export interface HostProbe {
  ok: boolean;
  message: string;
  uname: string | null;
  tmux: boolean;
  git: boolean;
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
  /// Short hostname of the box this daemon runs on. Absent on older
  /// daemons; clients fall back to the generic "this server" label.
  hostname?: string;
}

import {
  apiUrl,
  clearActiveToken,
  fetchProfile,
  getActiveProfile,
  setActiveToken,
  wsUrl as profileWsUrl
} from './profiles';
import { markFetchFailed, markFetchOk } from './stores/events';

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

  let res: Response;
  try {
    res = await fetch(apiUrl(path), { ...init, headers });
  } catch (e) {
    // Network-level failure (DNS, TCP refuse, TLS error, offline). Feed
    // the reconnect detector; the throw still bubbles for the caller.
    markFetchFailed();
    throw e;
  }
  if (res.status === 401) {
    // Stale or missing token — clear so the gate can re-prompt. Not a
    // reconnect signal: the daemon is reachable, the token isn't.
    setToken(null);
    throw new ApiError(401, 'unauthorized');
  }
  if (res.status >= 500) {
    // Daemon-side error counts as reachability failure for banner
    // purposes — the user's actions aren't landing anywhere useful.
    markFetchFailed();
    const text = await res.text().catch(() => '');
    throw new ApiError(res.status, text || res.statusText);
  }
  if (!res.ok) {
    // 4xx that isn't 401: legitimate client-side rejection, not a
    // network/health signal.
    const text = await res.text().catch(() => '');
    throw new ApiError(res.status, text || res.statusText);
  }
  markFetchOk();
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(`HTTP ${status}: ${message}`);
    this.name = 'ApiError';
  }
}

/**
 * Profile-pinned variant of `request()`. Used by call sites that need
 * to talk to a *specific* endpoint (e.g. the New Session dialog's
 * Servers picker, which spawns on the chosen server regardless of which
 * profile is "active" in the topbar). Falls back to the active profile
 * when `profileId` is empty / unknown — `fetchProfile` itself enforces
 * that contract — so callers can pass an empty string for "use the
 * current profile" without special-casing.
 */
async function requestOn<T>(
  profileId: string,
  path: string,
  init: RequestInit = {}
): Promise<T> {
  let res: Response;
  try {
    res = await fetchProfile(profileId, path, init);
  } catch (e) {
    markFetchFailed();
    throw e;
  }
  if (res.status === 401) {
    // Clearing the active token here would be wrong — the failing
    // request was for a *different* profile. The caller surfaces the
    // 401 inline; the per-profile login flow is the user's recourse.
    throw new ApiError(401, 'unauthorized');
  }
  if (res.status >= 500) {
    markFetchFailed();
    const text = await res.text().catch(() => '');
    throw new ApiError(res.status, text || res.statusText);
  }
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new ApiError(res.status, text || res.statusText);
  }
  markFetchOk();
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
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

/** Ticket type tag the kanban dot color picks up.
 *  'goal' is added for planner-spawned goal cards (lbl=goal); they render
 *  with the coral GOAL chip via .ticket .tk-foot .lbl.goal (UI-SPEC §lbl).
 */
export type TicketLbl = 'bug' | 'feat' | 'chore' | 'spike' | 'goal';
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
  /* Execution context (migration 0010): where the agent should run +
     optional model override. Carried so a ticket can be spawned into a
     session without re-asking. */
  workdir?: string | null;
  model?: string | null;
  /* Session linkage (migration 0011): when the user spawns a session
     from a ticket, the resulting session id is stamped here. */
  session_id?: string | null;
  /* Manual ordering within a column (migration 0012). Lower priority
     floats to the top; secondary sort is created_at ASC. */
  priority?: number;
  /* Goal linkage (migration 0015): child cards carry the parent goal id
     set by the planner when it decomposes a goal into tasks. Absent on
     goal cards themselves and on cards not part of any goal. */
  parent_goal_id?: number | null;
}

export interface NewBoardItem {
  title: string;
  body?: string | null;
  status?: string | null;
  lbl?: TicketLbl | null;
  tool?: Tool | null;
  workdir?: string | null;
  model?: string | null;
  session_id?: string | null;
  priority?: number | null;
}

export interface BoardPatch {
  title?: string;
  body?: string | null;
  status?: string;
  lbl?: TicketLbl | null;
  tool?: Tool | null;
  workdir?: string | null;
  model?: string | null;
  session_id?: string | null;
  priority?: number;
}

/** Threaded comment on a board item (migration 0013). */
export interface BoardComment {
  id: number;
  board_id: number;
  author: string;
  body: string;
  created_at: string;
}

export interface NewBoardComment {
  author: string;
  body: string;
}

/** One entry in the bulk reorder payload — id + new priority. */
export interface ReorderEntry {
  id: number;
  priority: number;
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
  /** Per-ticket comment count keyed by board id. Missing keys mean zero. */
  comment_counts?: Record<number, number>;
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

/** Snapshot of a session's pane — returned by GET /api/sessions/{id}/pane. */
export interface PaneSnapshot {
  lines: string[];
  captured_at: string; // RFC3339
}

/** Repo-relative path lists from `git status --porcelain` in a session's
 *  worktree (or workdir when no worktree is set). Returned by
 *  GET /api/sessions/{id}/git/status. Files with both staged + unstaged
 *  changes appear in BOTH `staged` and `unstaged` — render them as two
 *  rows so the user can act on either side. */
export interface GitStatus {
  staged: string[];
  unstaged: string[];
  untracked: string[];
}

/**
 * Plan-usage snapshot returned by `/api/usage`. Backend scans
 * `~/.claude/projects` (token sum over a 5h rolling window) and
 * `~/.codex/sessions` (most recent `token_count` event's
 * `rate_limits` block from the OpenAI response).
 */
export interface ClaudeUsage {
  window_tokens: number;
  window_start_ms: number | null;
  window_end_ms: number | null;
  all_time_tokens: number;
  by_model: Record<string, number>;
  claude_installed: boolean;
}

export interface CodexUsageWindow {
  used_percent: number;
  window_minutes: number;
  resets_at: number;
}

export interface CodexUsage {
  primary: CodexUsageWindow | null;
  secondary: CodexUsageWindow | null;
  plan_type: string | null;
  codex_installed: boolean;
}

export interface UsageBundle {
  claude: ClaudeUsage;
  codex: CodexUsage;
  generated_at_ms: number;
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
  /**
   * GET /api/sessions/{id}/pane — fetch the last `lines` rows of the
   * session's pane output. The optional `signal` is plumbed straight into
   * the underlying fetch so callers can abort in-flight requests on dialog
   * close or unmount. If `lines` is omitted, the server defaults to 20.
   */
  getSessionPane: (
    id: string,
    lines?: number,
    opts?: { signal?: AbortSignal },
  ): Promise<PaneSnapshot> => {
    const qs = lines !== undefined ? `?lines=${lines}` : '';
    const init: RequestInit = opts?.signal ? { signal: opts.signal } : {};
    return request<PaneSnapshot>(
      `/api/sessions/${encodeURIComponent(id)}/pane${qs}`,
      init,
    );
  },
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

  /**
   * Profile-pinned siblings of the session/listDir endpoints. Used by
   * the New Session dialog's Servers picker — the user chooses *where*
   * to spawn, so listDir (to pre-fill `$HOME`) and createSession both
   * need to target that server regardless of which profile is active
   * in the topbar.
   */
  listDirOn: (profileId: string, path?: string) => {
    const qs = path ? `?path=${encodeURIComponent(path)}` : '';
    return requestOn<DirListing>(profileId, `/api/fs/list${qs}`);
  },
  createSessionOn: (profileId: string, body: NewSession) =>
    requestOn<Session>(profileId, '/api/sessions', {
      method: 'POST',
      body: JSON.stringify(body)
    }),
  startSessionOn: (profileId: string, id: string) =>
    requestOn<Session>(profileId, `/api/sessions/${encodeURIComponent(id)}/start`, {
      method: 'POST'
    }),
  listAgentsOn: (profileId: string, hostId?: string | null) => {
    const qs = hostId ? `?host_id=${encodeURIComponent(hostId)}` : '';
    return requestOn<AgentInfo[]>(profileId, `/api/agents${qs}`);
  },
  listHosts: () => request<Host[]>('/api/hosts'),
  listHostsOn: (profileId: string) => requestOn<Host[]>(profileId, '/api/hosts'),
  createHost: (body: NewHost) =>
    request<Host>('/api/hosts', { method: 'POST', body: JSON.stringify(body) }),
  testHost: (id: string) =>
    request<HostProbe>(`/api/hosts/${encodeURIComponent(id)}/test`, { method: 'POST' }),
  deleteHost: (id: string) =>
    request<void>(`/api/hosts/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  listDirHostOn: (profileId: string, hostId: string | null | undefined, path?: string) => {
    const qs = new URLSearchParams();
    if (path) qs.set('path', path);
    if (hostId) qs.set('host_id', hostId);
    const suffix = qs.toString() ? `?${qs.toString()}` : '';
    return requestOn<DirListing>(profileId, `/api/fs/list${suffix}`);
  },
  sendInput: (id: string, body: SendInput) =>
    request<void>(`/api/sessions/${encodeURIComponent(id)}/send`, {
      method: 'POST',
      body: JSON.stringify(body)
    }),

  // ---------- git (ORCA §3 diff viewer) ----------
  /** GET /api/sessions/{id}/git/status — repo-relative path lists. */
  gitStatus: (id: string) =>
    request<GitStatus>(`/api/sessions/${encodeURIComponent(id)}/git/status`),
  /** GET /api/sessions/{id}/git/diff?path=&staged= — unified diff text.
   *  The server returns `text/plain`, so we bypass the JSON-parsing
   *  `request()` helper and read the response body verbatim. */
  async gitDiff(id: string, path: string, staged = false): Promise<string> {
    const qs = new URLSearchParams({ path, staged: String(staged) }).toString();
    const headers = new Headers();
    const token = readToken();
    if (token) headers.set('authorization', `Bearer ${token}`);
    const res = await fetch(
      apiUrl(`/api/sessions/${encodeURIComponent(id)}/git/diff?${qs}`),
      { headers }
    );
    if (res.status === 401) {
      setToken(null);
      throw new ApiError(401, 'unauthorized');
    }
    if (!res.ok) {
      const text = await res.text().catch(() => '');
      throw new ApiError(res.status, text || res.statusText);
    }
    return res.text();
  },
  /** POST /api/sessions/{id}/git/commit — stages + commits the listed
   *  paths in one shot. The server pins the author identity
   *  (`agentum-bot <agentum@localhost>`) so worktrees that inherit no
   *  gitconfig still commit cleanly. */
  gitCommit: (id: string, message: string, paths: string[]) =>
    request<{ sha: string }>(
      `/api/sessions/${encodeURIComponent(id)}/git/commit`,
      { method: 'POST', body: JSON.stringify({ message, paths }) }
    ),

  // ---------- usage (Claude/Codex plan headroom) ----------
  /** GET /api/usage — plan-usage snapshot for the sidebar chip. */
  getUsage: () => request<UsageBundle>('/api/usage'),

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
  releaseBoardItem: (id: number, claimed_by: string) =>
    request<BoardItem>(`/api/board/${id}/release`, {
      method: 'POST',
      body: JSON.stringify({ claimed_by })
    }),
  /* ---- profile-pinned variants for the multi-server fleet board ---- */
  listBoardOn: (profileId: string) =>
    requestOn<GroupedBoard>(profileId, '/api/board'),
  createBoardItemOn: (profileId: string, body: NewBoardItem) =>
    requestOn<BoardItem>(profileId, '/api/board', {
      method: 'POST',
      body: JSON.stringify(body)
    }),
  patchBoardItemOn: (profileId: string, id: number, body: BoardPatch) =>
    requestOn<BoardItem>(profileId, `/api/board/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(body)
    }),
  /** GET /api/board/{id} — fetch a single board item by numeric id. */
  getBoardItem: (id: number) =>
    request<BoardItem>(`/api/board/${id}`),
  /** Profile-pinned variant of getBoardItem for multi-server fleets. */
  getBoardItemOn: (profileId: string, id: number) =>
    requestOn<BoardItem>(profileId, `/api/board/${id}`),
  deleteBoardItemOn: (profileId: string, id: number) =>
    requestOn<void>(profileId, `/api/board/${id}`, { method: 'DELETE' }),
  claimBoardItemOn: (profileId: string, id: number, claimed_by: string) =>
    requestOn<BoardItem>(profileId, `/api/board/${id}/claim`, {
      method: 'POST',
      body: JSON.stringify({ claimed_by })
    }),
  releaseBoardItemOn: (profileId: string, id: number, claimed_by: string) =>
    requestOn<BoardItem>(profileId, `/api/board/${id}/release`, {
      method: 'POST',
      body: JSON.stringify({ claimed_by })
    }),
  /* ---- comments + reorder (migrations 0012 + 0013) ---- */
  listBoardCommentsOn: (profileId: string, id: number) =>
    requestOn<BoardComment[]>(profileId, `/api/board/${id}/comments`),
  createBoardCommentOn: (profileId: string, id: number, body: NewBoardComment) =>
    requestOn<BoardComment>(profileId, `/api/board/${id}/comments`, {
      method: 'POST',
      body: JSON.stringify(body)
    }),
  reorderBoardOn: (profileId: string, entries: ReorderEntry[]) =>
    requestOn<void>(profileId, '/api/board/reorder', {
      method: 'POST',
      body: JSON.stringify({ entries })
    }),
  claimBoardItem: (id: number, claimed_by: string) =>
    request<BoardItem>(`/api/board/${id}/claim`, {
      method: 'POST',
      body: JSON.stringify({ claimed_by })
    }),

  /**
   * POST /api/board/goals — atomically creates a goal card and spawns the
   * planner session. Returns the new goal BoardItem + the planner session
   * id (empty string when spawn failed — per D-07, the goal card is still
   * created and the daemon emits goal.planner.spawn_failed on the bus).
   *
   * The caller (GoalComposer) does NOT optimistically insert a card: the
   * WS event bridge delivers goal.created / board.created within ~1s, which
   * is the source of truth. Racing a placeholder would cause a visible
   * flicker when the server-assigned id/key differ from any synthetic id.
   */
  createGoal: (
    text: string,
    opts?: { body?: string; workdir?: string }
  ): Promise<{ goal: BoardItem; planner_session_id: string }> =>
    request('/api/board/goals', {
      method: 'POST',
      body: JSON.stringify({
        title: text,
        ...(opts?.body ? { body: opts.body } : {}),
        ...(opts?.workdir ? { workdir: opts.workdir } : {})
      })
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
