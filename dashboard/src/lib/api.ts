/**
 * Thin fetch wrapper for the agentum HTTP API.
 *
 * Bearer token plumbing is in place but unused in phase 3 (auth middleware
 * lands phase 5). When the token slot is empty, requests go out unauth'd
 * and the backend allows them.
 */

export type Status = 'idle' | 'running' | 'stopped' | 'crashed';

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
}

export interface NewSession {
  name: string;
  workdir: string;
  tool: string;
  model?: string | null;
  flags?: string[];
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
}

const TOKEN_KEY = 'agentum_token';

function readToken(): string | null {
  if (typeof localStorage === 'undefined') return null;
  return localStorage.getItem(TOKEN_KEY);
}

export function setToken(token: string | null) {
  if (typeof localStorage === 'undefined') return;
  if (token === null) localStorage.removeItem(TOKEN_KEY);
  else localStorage.setItem(TOKEN_KEY, token);
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  if (!headers.has('content-type') && init.body) {
    headers.set('content-type', 'application/json');
  }
  const token = readToken();
  if (token) headers.set('authorization', `Bearer ${token}`);

  const res = await fetch(path, { ...init, headers });
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

export interface SendInput {
  text?: string;
  keys?: string;
  append_enter?: boolean;
}

export interface BoardItem {
  id: number;
  key: string;
  title: string;
  body: string | null;
  status: string;
  claimed_by: string | null;
  created_at: string;
  updated_at: string;
}

export interface NewBoardItem {
  title: string;
  body?: string | null;
  status?: string | null;
}

export interface BoardPatch {
  title?: string;
  body?: string | null;
  status?: string;
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
  patchSession: (id: string, body: { flags?: string[]; model?: string | null }) =>
    request<Session>(`/api/sessions/${encodeURIComponent(id)}`, { method: 'PATCH', body: JSON.stringify(body) }),
  deleteSession: (id: string, force = false) =>
    request<void>(
      `/api/sessions/${encodeURIComponent(id)}${force ? '?force=true' : ''}`,
      { method: 'DELETE' }
    ),
  doctor: () => request<DoctorReport>('/api/doctor'),

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
   * Open a WebSocket to the session's pane stream. Caller is responsible for
   * closing it. Resolves the URL relative to the current origin so dev (vite
   * proxy) and prod (embedded SPA) both work. Browsers cannot set custom
   * headers on WS upgrades, so the bearer token is passed as a `?token=`
   * query param (the backend accepts both forms).
   */
  streamUrl(id: string): string {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const base = `${proto}//${location.host}/api/sessions/${encodeURIComponent(id)}/stream`;
    const token = readToken();
    return token ? `${base}?token=${encodeURIComponent(token)}` : base;
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
