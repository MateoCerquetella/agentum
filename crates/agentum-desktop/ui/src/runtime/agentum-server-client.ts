// Typed client for the embedded agentum-server — the same HTTP/WS core the TUI
// drives. Session-per-workspace (Option A): repos/worktrees become server
// sessions and git/fs/terminal flow through /api/sessions/{id}/*. Built on the
// loopback endpoint resolved in server-endpoint.ts. Mirrors the TUI client in
// crates/agentum-tui/src/commands/terminal/api.rs.
import { apiUrl, wsUrl, getServerEndpoint } from './server-endpoint'
import { reconnectBackoffMs as backoffMs } from './reconnect-backoff'
import { record as recordHostIo, LOCAL_HOST_KEY, type HostKey } from './io-meter'

type SessionStatus = 'idle' | 'running' | 'stopped' | 'crashed'

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
  /** Isolated worktree checkout for this session, when one was provisioned
   *  (e.g. a board card-start). Absent → the agent runs in `workdir`. */
  worktree_path?: string | null
  worktree_branch?: string | null
}

// Wire shape of /api/agents (AgentInfo).
type AgentInfo = {
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
  /** Fired only when the stream is GONE FOR GOOD: the session no longer exists
   *  on the server (404) or our token was rejected (401/403). A transient drop
   *  — a flaky `ssh tail`, sshd channel pressure, or a cold ControlMaster right
   *  after the app restarts — does NOT fire this; it triggers `onReconnecting`
   *  and an automatic retry, because the pane lives in tmux on the server and a
   *  reconnect re-attaches to it. */
  onClose?: () => void
  onError?: (event: Event) => void
  /** Fired once per reconnect attempt while the stream is dropped but
   *  recoverable (`attempt` starts at 1). A successful reconnect makes the
   *  server replay a fresh snapshot, repainting the pane — so callers only need
   *  to show a transient "reconnecting…" hint, not tear anything down. */
  onReconnecting?: (attempt: number) => void
  /** Fired when a connection opens AFTER at least one drop — i.e. the stream
   *  RECOVERED. A live re-attach proves the host is reachable again, which the
   *  desktop uses to flip the SSH status badge back to connected and refresh
   *  host-scoped surfaces (the file tree) that failed during the outage. Not
   *  fired on the first connect, which is not a recovery. */
  onReconnected?: () => void
}

/** Handle for an open terminal stream. */
export type SessionStream = {
  /** Send raw keystrokes to the pane (binary frame → `tmux send-keys -H`). */
  send: (data: Uint8Array | string) => void
  /** Resize the pane (JSON text frame the server forwards to `resize-window`). */
  resize: (cols: number, rows: number) => void
  /** Force a fresh full re-snapshot: drop the current socket and reconnect
   *  WITHOUT `resume`, so the server repaints the whole pane. The blank-pane
   *  self-heal calls this when a connected stream painted nothing (a snapshot
   *  lost in a client reflow, or an empty server snapshot under SSH channel
   *  pressure) — for an idle remote pane there are no live bytes to recover it
   *  otherwise. Silent: it does NOT count as a reconnect (no onReconnecting). */
  requestRepaint: () => void
  /** Like {@link requestRepaint}, but also asks the server to make the agent
   *  fully REPAINT (a SIGWINCH nudge, `?redraw=true`) before snapshotting —
   *  not just re-read the current grid. Use when the pane grid itself is
   *  corrupted by bytes the agent didn't draw, which a plain re-snapshot would
   *  faithfully reproduce: an OS `wall` broadcast (systemd's "system will
   *  suspend now!" notice) written over the input box, or a half-painted
   *  frame. Fired automatically on reconnect (the suspend/resume path) and by
   *  the manual "force redraw" shortcut. Old daemons ignore the param and fall
   *  back to a plain repaint. */
  requestRedraw: () => void
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
function listAgents(): Promise<AgentInfo[]> {
  return request<AgentInfo[]>('/api/agents')
}

/** Wire shape of `/api/orchestration/settings`. */
export type OrchestrationSettings = { enabled: boolean }

/**
 * `GET /api/orchestration/settings` — is agentum's orchestration MCP surface
 * (inter-agent mailbox + task DAG) turned on? The server-side gate read by
 * `routes/mcp.rs` at tools/list + tools/call, so this is the source of truth the
 * Settings toggle reflects (not localStorage).
 */
export function getOrchestrationSettings(): Promise<OrchestrationSettings> {
  return request<OrchestrationSettings>('/api/orchestration/settings')
}

/** `PUT /api/orchestration/settings` — turn the orchestration MCP surface on/off. */
export function setOrchestrationSettings(enabled: boolean): Promise<OrchestrationSettings> {
  return request<OrchestrationSettings>('/api/orchestration/settings', {
    method: 'PUT',
    body: JSON.stringify({ enabled })
  })
}

/** Wire shape of `/api/mcp/settings`. */
export type McpSettings = { enabled: boolean }

/**
 * `GET /api/mcp/settings` — is agentum's MCP wired into the agents agentum
 * launches? The master switch (default on), read at provision time by
 * `mcp_provision::provision`. Source of truth for the Settings → Agent MCP toggle.
 */
export function getMcpSettings(): Promise<McpSettings> {
  return request<McpSettings>('/api/mcp/settings')
}

/** `PUT /api/mcp/settings` — flip the agentum-MCP master switch. */
export function setMcpSettings(enabled: boolean): Promise<McpSettings> {
  return request<McpSettings>('/api/mcp/settings', {
    method: 'PUT',
    body: JSON.stringify({ enabled })
  })
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

/** `GET /api/sessions/{id}` — one session by id. */
export function getSession(id: string): Promise<Session> {
  return request<Session>(`/api/sessions/${id}`)
}

/** Wire shape of `POST /api/sessions/{id}/uploads`. */
export type UploadResponse = { path: string; relative_path: string; size_bytes: number }

/**
 * `POST /api/sessions/{id}/uploads` — upload raw image bytes for a session. The
 * (host-aware) server writes them into the session's workdir — on the REMOTE
 * host over SSH for a remote session — and types the relative path into the
 * pane (no Enter) so the agent attaches the screenshot. This is how the desktop
 * delivers a pasted/dropped image to an SSH agent, where a local temp path is
 * unreachable. `Content-Type` selects the extension (image/png|jpeg|gif|webp|bmp).
 */
export function uploadSessionImage(
  id: string,
  bytes: Uint8Array | ArrayBuffer | Blob,
  contentType = 'image/png'
): Promise<UploadResponse> {
  return request<UploadResponse>(`/api/sessions/${id}/uploads`, {
    method: 'POST',
    body: bytes as BodyInit,
    headers: { 'Content-Type': contentType }
  })
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

/** `POST /api/sessions/{id}/start` — bring the tmux pane up. The response is
 *  the session plus `spawned`: `true` for a freshly created pane (bare shell),
 *  `false` when start reattached to a live tmux session that still runs
 *  whatever was in it (possibly an agent). */
export function startSession(id: string): Promise<Session & { spawned?: boolean }> {
  return request<Session & { spawned?: boolean }>(`/api/sessions/${id}/start`, { method: 'POST' })
}

/** `POST /api/sessions/{id}/stop` — graceful stop (pane survives). */
function stopSession(id: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/stop`, { method: 'POST' })
}

/** `POST /api/sessions/{id}/kill` — kill the tmux pane. */
function killSession(id: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/kill`, { method: 'POST' })
}

/** `POST /api/sessions/{id}/send` — inject text and/or a raw tmux key spec. */
function sendToSession(
  id: string,
  payload: { text?: string; keys?: string }
): Promise<void> {
  return request<void>(`/api/sessions/${id}/send`, {
    method: 'POST',
    body: JSON.stringify(payload)
  })
}

/** `POST /api/sessions/{id}/submit` — deliver a prompt to a RUNNING agent's REPL and
 *  submit it robustly: the server types the body, waits for the paste to settle, then
 *  sends a SEPARATE Enter so a multi-line prompt isn't swallowed as a "[Pasted text]"
 *  block. Use this (not `sendToSession`) for "send to an agent"; it reaches any session
 *  on the worktree, including tmux/MCP-spawned agents never opened as terminal tabs. */
export function submitPromptToSession(id: string, text: string): Promise<void> {
  return request<void>(`/api/sessions/${id}/submit`, {
    method: 'POST',
    body: JSON.stringify({ text })
  })
}

/** `PATCH /api/sessions/{id}` — rename (pure metadata; allowed while running). */
function renameSession(id: string, name: string): Promise<Session> {
  return request<Session>(`/api/sessions/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ name })
  })
}

/** `DELETE /api/sessions/{id}` — remove; `force` also kills a running pane. */
function deleteSession(id: string, force = false): Promise<void> {
  return request<void>(`/api/sessions/${id}${force ? '?force=true' : ''}`, { method: 'DELETE' })
}

/** Is this session permanently unstreamable — deleted (404) or our token
 *  rejected (401/403)? The reconnect loop calls this to decide whether to keep
 *  retrying. A network error reaching the API is NOT terminal (the API may be
 *  momentarily unreachable while the remote pane is perfectly fine), so we
 *  return false and let the WS reconnect keep trying in that case. */
async function sessionGoneOrUnauthorized(id: string): Promise<boolean> {
  try {
    const url = await apiUrl(`/api/sessions/${id}`)
    const res = await fetch(url, { headers: await authHeaders() })
    return res.status === 401 || res.status === 403 || res.status === 404
  } catch {
    return false
  }
}

/**
 * Open the bidirectional terminal stream for a session
 * (`WS /api/sessions/{id}/stream`), and KEEP IT OPEN across transient drops.
 *
 * Server → client frames are raw pane bytes; client → server frames are binary
 * (keystrokes) or a `{"resize":{cols,rows}}` text frame. The current size is
 * (re)sent on every connect so the server sizes the freshly-attached pane.
 *
 * Resilience — this is the whole reason the TUI survives SSH hiccups and the
 * desktop used to not: the pane lives in tmux on the server, so a dropped WS is
 * recoverable. We transparently reconnect with capped exponential backoff and
 * the server re-attaches and repaints the current pane (the remote path always
 * re-snapshots; the local path replays the resume delta). We only give up (fire
 * `onClose`) when the session is genuinely gone or our token is rejected. The
 * returned handle is STABLE across reconnects — `send`/`resize` always target
 * the live socket — so the caller binds its xterm listeners exactly once.
 * Mirrors the TUI's `open_terminal_stream` reconnect loop (terminal/api.rs).
 */
export async function openSessionStream(
  id: string,
  initial: { cols: number; rows: number },
  handlers: SessionStreamHandlers,
  // Which host bucket this stream's WS bytes count toward (status-bar I/O meter).
  // Omitted → local host. This is the per-host data rate the TUI also shows.
  hostKey: HostKey = LOCAL_HOST_KEY
): Promise<SessionStream> {
  const { token } = await getServerEndpoint()
  const base = await wsUrl(`/api/sessions/${id}/stream`)

  let ws: WebSocket | null = null
  let disposed = false
  // After the first successful attach, ask the server to replay only the bytes
  // we missed instead of a full snapshot. Local sessions honor this via the
  // resume checkpoint; the remote path ignores the flag and re-snapshots — a
  // clean repaint either way.
  let connectedOnce = false
  // Set by `requestRepaint` to force the NEXT connect to fetch a full fresh
  // snapshot (omit `resume`), so a blank-pane self-heal actually repaints
  // instead of replaying an empty delta on the local path. Consumed once.
  let forceFreshNext = false
  // Set by `requestRedraw` to add `?redraw=true` to the NEXT connect, asking
  // the server to make the agent fully repaint (a SIGWINCH nudge) before
  // snapshotting — heals a corrupted grid (e.g. a suspend `wall` broadcast
  // written over the pane) that a plain re-snapshot would just re-capture.
  // Consumed once, alongside `forceFreshNext` (a redraw always wants a fresh
  // snapshot, never a resume delta).
  let redrawNext = false
  let attempt = 0
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  let stableTimer: ReturnType<typeof setTimeout> | null = null
  // Latest size the caller asked for, re-sent on every (re)connect.
  let lastCols = initial.cols
  let lastRows = initial.rows

  const sendResize = (cols: number, rows: number): void => {
    lastCols = cols
    lastRows = rows
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ resize: { cols, rows } }))
    }
  }

  const streamUrl = (): string => {
    const params = new URLSearchParams()
    if (token) {
      params.set('token', token)
    }
    if (connectedOnce && !forceFreshNext) {
      params.set('resume', 'true')
    }
    if (redrawNext) {
      params.set('redraw', 'true')
    }
    const qs = params.toString()
    return qs ? `${base}?${qs}` : base
  }

  const clearTimers = (): void => {
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    if (stableTimer) {
      clearTimeout(stableTimer)
      stableTimer = null
    }
  }

  const scheduleReconnect = (): void => {
    if (disposed || reconnectTimer) {
      return
    }
    attempt += 1
    handlers.onReconnecting?.(attempt)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      if (disposed) {
        return
      }
      // The browser WS API hides a failed upgrade's HTTP status, so the close
      // event alone can't separate "session deleted / bad token" (give up) from
      // "network blip" (keep trying). Probe the REST endpoint, which DOES expose
      // the status — mirrors the TUI giving up on 401/403/404.
      void sessionGoneOrUnauthorized(id).then((fatal) => {
        if (disposed) {
          return
        }
        if (fatal) {
          disposed = true
          clearTimers()
          handlers.onClose?.()
          return
        }
        connect()
      })
    }, backoffMs(attempt))
  }

  const connect = (): void => {
    if (disposed) {
      return
    }
    const url = streamUrl()
    forceFreshNext = false // consumed for this connect
    redrawNext = false // consumed for this connect
    const sock = new WebSocket(url)
    sock.binaryType = 'arraybuffer'
    ws = sock

    sock.addEventListener('open', () => {
      // `attempt >= 1` means this open follows ≥1 drop → we RECOVERED. Fire
      // before the stable timer resets `attempt`, so the desktop can re-mark the
      // host connected and refresh surfaces that failed during the outage. The
      // first connect has `attempt === 0`, so recovery never fires spuriously.
      if (attempt >= 1) {
        handlers.onReconnected?.()
      }
      connectedOnce = true
      sendResize(lastCols, lastRows)
      // Only treat this as a healthy connection (resetting backoff) once it has
      // held briefly — otherwise a flapping link resets backoff on every open
      // and reconnects in a tight loop.
      if (stableTimer) {
        clearTimeout(stableTimer)
      }
      stableTimer = setTimeout(() => {
        attempt = 0
      }, 3000)
    })
    sock.addEventListener('message', (event) => {
      if (event.data instanceof ArrayBuffer) {
        // Inbound pane bytes — meter before handing off to the renderer.
        recordHostIo(hostKey, { in: event.data.byteLength })
        handlers.onData(new Uint8Array(event.data))
      }
      // Text frames are control/no-op for the renderer; ignore.
    })
    sock.addEventListener('close', () => {
      if (sock !== ws) {
        return // superseded by a newer socket
      }
      if (stableTimer) {
        clearTimeout(stableTimer)
        stableTimer = null
      }
      if (disposed) {
        return // caller tore us down; not an error
      }
      scheduleReconnect()
    })
    sock.addEventListener('error', (event) => handlers.onError?.(event))
  }

  connect()

  return {
    send: (data) => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        const frame = typeof data === 'string' ? new TextEncoder().encode(data) : data
        // Outbound keystrokes/data over the same WS — meter the wire bytes.
        recordHostIo(hostKey, { out: frame.byteLength })
        ws.send(frame)
      }
    },
    resize: sendResize,
    requestRepaint: () => {
      if (disposed) {
        return
      }
      // Next connect must fetch a full snapshot, not a resume delta.
      forceFreshNext = true
      // Supersede the current socket BEFORE closing it: with `ws` cleared, the
      // old socket's close handler sees `sock !== ws` and bails early, so this
      // does NOT schedule a backoff reconnect or fire onReconnecting — it's a
      // silent in-place repaint, not a recovery. Then connect fresh now.
      const old = ws
      ws = null
      try {
        old?.close()
      } catch {
        // already closing/closed — connect() below still re-establishes
      }
      connect()
    },
    requestRedraw: () => {
      if (disposed) {
        return
      }
      // Same silent in-place reconnect as requestRepaint (no backoff, no
      // onReconnecting), but with `?redraw=true` so the server forces the
      // agent to fully repaint before snapshotting. A redraw always wants the
      // fresh snapshot, never a resume delta — so set forceFreshNext too.
      forceFreshNext = true
      redrawNext = true
      const old = ws
      ws = null
      try {
        old?.close()
      } catch {
        // already closing/closed — connect() below still re-establishes
      }
      connect()
    },
    close: () => {
      disposed = true
      clearTimers()
      ws?.close()
    }
  }
}
