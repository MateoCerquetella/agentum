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

export interface SendInput {
  text?: string;
  keys?: string;
  append_enter?: boolean;
}

export const api = {
  health: () => request<Health>('/api/health'),
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
  deleteSession: (id: string) =>
    request<void>(`/api/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  sendInput: (id: string, body: SendInput) =>
    request<void>(`/api/sessions/${encodeURIComponent(id)}/send`, {
      method: 'POST',
      body: JSON.stringify(body)
    }),

  /**
   * Open a WebSocket to the session's pane stream. Caller is responsible for
   * closing it. Resolves the URL relative to the current origin so dev (vite
   * proxy) and prod (embedded SPA) both work.
   */
  streamUrl(id: string): string {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${location.host}/api/sessions/${encodeURIComponent(id)}/stream`;
  }
};
