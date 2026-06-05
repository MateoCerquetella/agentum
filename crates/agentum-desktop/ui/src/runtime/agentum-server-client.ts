// Typed client for the embedded agentum-server — the same HTTP/WS core the TUI
// drives. Session-per-workspace (Option A): repos/worktrees become server
// sessions and git/fs/terminal flow through /api/sessions/{id}/*. Built on the
// loopback endpoint resolved in server-endpoint.ts. Mirrors the TUI client in
// crates/agentum-cli/src/commands/terminal/api.rs.
import { apiUrl, wsUrl, getServerEndpoint } from './server-endpoint'

export type SessionStatus = 'idle' | 'running' | 'stopped' | 'crashed'

// Wire shape of agentum_core::Session (serde snake_case). Kept faithful to the
// server JSON so there is one source of truth and no silent field drift.
export type Session = {
  id: string
  name: string
  workdir: string
  tool: string
  model?: string | null
  flags: string[]
  status: SessionStatus
  tmux_target?: string | null
  host_id?: string | null
  host_label?: string | null
  host_kind?: string | null
  created_at: string
  updated_at: string
  last_activity_at?: string | null
  tokens?: number | null
  cost?: number | null
}

// Wire shape of /api/agents (AgentInfo).
export type AgentInfo = {
  name: string
  binary: string
  available: boolean
  yolo_flag?: string | null
  path?: string | null
}

export type CreateSessionInput = {
  name: string
  workdir: string
  tool: string
  model?: string
  flags?: string[]
  /** Ask the server to `git worktree add` a dedicated branch for this session. */
  worktree?: boolean
}

/** Bytes from the tmux pane (server → client) for a live terminal surface. */
export type SessionStreamHandlers = {
  onData: (bytes: Uint8Array) => void
  onClose?: () => void
  onError?: (event: Event) => void
}

/** Handle for an open terminal stream. */
export type SessionStream = {
  /** Send raw keystrokes to the pane (binary frame → `tmux send-keys -H`). */
  send: (data: Uint8Array | string) => void
  /** Resize the pane (JSON text frame the server forwards to `resize-window`). */
  resize: (cols: number, rows: number) => void
  close: () => void
}

async function authHeaders(): Promise<Record<string, string>> {
  const { token } = await getServerEndpoint()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const url = await apiUrl(path)
  const res = await fetch(url, {
    ...init,
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...(await authHeaders()),
      ...(init?.headers ?? {})
    }
  })
  if (!res.ok) {
    const detail = await res.text().catch(() => '')
    throw new Error(`agentum-server ${res.status} on ${path}${detail ? ` — ${detail}` : ''}`)
  }
  // 2xx with empty body (action endpoints) → undefined.
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

/** `GET /api/agents` — which first-class agent binaries are installed. */
export function listAgents(): Promise<AgentInfo[]> {
  return request<AgentInfo[]>('/api/agents')
}

/**
 * Boot-time smoke check: confirm the embedded server answers a real
 * session-model round-trip (not just /api/health) and log what it sees.
 */
export async function logEmbeddedServerSnapshot(): Promise<void> {
  try {
    const [agents, sessions] = await Promise.all([listAgents(), listSessions()])
    const available = agents.filter((a) => a.available).map((a) => a.name)
    console.info(
      `[agentum] embedded server ready — ${sessions.length} session(s); ` +
        `agents available: ${available.length ? available.join(', ') : 'none'}`
    )
  } catch (error) {
    console.warn('[agentum] embedded server session-model snapshot failed:', error)
  }
}

/** `GET /api/sessions` — all sessions known to the daemon. */
export function listSessions(): Promise<Session[]> {
  return request<Session[]>('/api/sessions')
}

/** `POST /api/sessions` — create a session (optionally in a dedicated worktree). */
export function createSession(input: CreateSessionInput): Promise<Session> {
  const body: Record<string, unknown> = {
    name: input.name,
    workdir: input.workdir,
    tool: input.tool
  }
  if (input.model) body.model = input.model
  if (input.flags && input.flags.length > 0) body.flags = input.flags
  // Run the agent's tmux pane on a specific server host (SSH = remote). Omitted
  // → server defaults to the local host. Was declared but never forwarded.
  if (input.host_id) body.host_id = input.host_id
  // Empty WorktreeSpec: branch derives from the session name, base ref = HEAD.
  if (input.worktree) body.worktree = {}
  return request<Session>('/api/sessions', { method: 'POST', body: JSON.stringify(body) })
}

/** `POST /api/sessions/{id}/start` — bring the tmux pane up. */
export function startSession(id: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/start`, { method: 'POST' })
}

/** `POST /api/sessions/{id}/stop` — graceful stop (pane survives). */
export function stopSession(id: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/stop`, { method: 'POST' })
}

/** `POST /api/sessions/{id}/kill` — kill the tmux pane. */
export function killSession(id: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/kill`, { method: 'POST' })
}

/** `POST /api/sessions/{id}/send` — inject text and/or a raw tmux key spec. */
export function sendToSession(
  id: string,
  payload: { text?: string; keys?: string }
): Promise<void> {
  return request<void>(`/api/sessions/${id}/send`, {
    method: 'POST',
    body: JSON.stringify(payload)
  })
}

/** `PATCH /api/sessions/{id}` — rename (pure metadata; allowed while running). */
export function renameSession(id: string, name: string): Promise<Session> {
  return request<Session>(`/api/sessions/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ name })
  })
}

/** `DELETE /api/sessions/{id}` — remove; `force` also kills a running pane. */
export function deleteSession(id: string, force = false): Promise<void> {
  return request<void>(`/api/sessions/${id}${force ? '?force=true' : ''}`, { method: 'DELETE' })
}

/**
 * Open the bidirectional terminal stream for a session
 * (`WS /api/sessions/{id}/stream`). Server → client frames are raw pane bytes;
 * client → server frames are binary (keystrokes) or a `{"resize":{cols,rows}}`
 * text frame. The initial resize is sent on open so the server sizes the pane.
 */
export async function openSessionStream(
  id: string,
  initial: { cols: number; rows: number },
  handlers: SessionStreamHandlers
): Promise<SessionStream> {
  const { token } = await getServerEndpoint()
  const base = await wsUrl(`/api/sessions/${id}/stream`)
  const url = token ? `${base}?token=${encodeURIComponent(token)}` : base
  const ws = new WebSocket(url)
  ws.binaryType = 'arraybuffer'

  const sendResize = (cols: number, rows: number): void => {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ resize: { cols, rows } }))
    }
  }

  ws.addEventListener('open', () => sendResize(initial.cols, initial.rows))
  ws.addEventListener('message', (event) => {
    if (event.data instanceof ArrayBuffer) {
      handlers.onData(new Uint8Array(event.data))
    }
    // Text frames are control/no-op for the renderer; ignore.
  })
  ws.addEventListener('close', () => handlers.onClose?.())
  ws.addEventListener('error', (event) => handlers.onError?.(event))

  return {
    send: (data) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(typeof data === 'string' ? new TextEncoder().encode(data) : data)
      }
    },
    resize: sendResize,
    close: () => ws.close()
  }
}
