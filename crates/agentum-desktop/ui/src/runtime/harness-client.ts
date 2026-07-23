// Typed client for the Harness Engine routes on the embedded agentum-server
// (`/api/harness/*`). Mirrors `agentum-server-client.ts`: built on the loopback
// endpoint resolved in `server-endpoint.ts`. The wire shapes here are kept
// faithful to `crates/agentum-server/src/harness.rs` (serde snake_case) so there
// is one source of truth and no silent field drift.
import { apiUrl, wsUrl, getServerEndpoint } from './server-endpoint'
import { reconnectBackoffMs as backoffMs } from './reconnect-backoff'

export type FeatureState =
  | 'pending'
  | 'coding'
  | 'verifying'
  | 'ready_to_test'
  | 'awaiting_confirm'
  | 'done'
  | 'blocked'

type HarnessState =
  | 'idle'
  | 'init_verifying'
  | 'running'
  | 'verifying'
  | 'awaiting_confirmation'
  | 'blocked'
  | 'done'
  | 'failed'

/** SDD phase a run is in, layered above the feature backlog (spec 013). */
export type SpecPhase =
  | 'authoring'
  | 'architecture'
  | 'decompose'
  | 'executing'
  | 'review'
  | 'done'
  | 'blocked'
  | 'awaiting_confirm'

/** The SDD role behind an agent-played gate (spec 013). */
type RoleKind = 'pm' | 'architect' | 'reviewer'

export type HarnessFeature = {
  id: string
  name: string
  description: string
  state: FeatureState
  attempts: number
  last_error?: string | null
  prompt?: string | null
  /** External task tracker this feature mirrors (`github` / `linear`). */
  tracker_provider?: string | null
  /** The external tracker item's URL. */
  tracker_url?: string | null
}

export type HarnessFeatureList = {
  features: HarnessFeature[]
  max_retries: number
  agent_tool: string
  agent_model?: string | null
  settle_grace_secs: number
  settle_timeout_secs: number
  agent_yolo: boolean
  hitl_at_qa?: boolean
  /** How the browser QA gate runs (spec 012b). */
  qa_mode?: 'auto' | 'script' | 'agent'
  /** Agent CLI for the QA gate when it spawns one (default: the feature agent). */
  qa_agent_tool?: string | null
  /** SDD spec id to author + decompose when `roles` is on (spec 013). */
  spec_id?: string | null
  /** Run the SDD role-gate phases around the feature loop (spec 013). */
  roles?: boolean
  /** Pause (vs. block) when a role gate exhausts retries (spec 013). */
  hitl_on_block?: boolean
  /** Missing/sequential preserves the compatibility WIP=1 engine. */
  execution_mode?: 'sequential' | 'orchestrated'
  max_concurrency?: number
}

export type HarnessWorkerStatus = {
  task_id: string
  state: 'pending' | 'ready' | 'dispatched' | 'working' | 'patch_pending' | 'verifying' | 'completed' | 'blocked'
  session_id?: string | null
  enforcement: 'enforced' | 'best_effort' | string
  context_remaining?: number | null
  patch_state?: string | null
  conflict?: string | null
}

// Exported for the spec-023 surfaces (GatedRunBar / useWorktreeHarnessRun /
// lib/harness-run.ts): the wire shapes stay faithful to
// `crates/agentum-server/src/harness.rs` (serde snake_case).
export type HarnessStatus = {
  id: string
  workdir: string
  /** Authoritative workspace identity. Missing only on legacy local runs. */
  worktree_id?: string | null
  repo_id?: string | null
  /** Server host id pinned when the run was registered. */
  host_id?: string | null
  state: HarnessState
  features: HarnessFeatureList
  current_feature?: string | null
  current_session?: string | null
  /** Validated by the UI before becoming a tab launch-agent hint. */
  current_agent_tool?: string | null
  elapsed_secs: number
  agent_instructions: string
  /** Current SDD phase (spec 013); `executing` for a plain feature run. */
  phase?: SpecPhase
  /** Role-gate retry counter for the current phase (spec 013). */
  phase_attempts?: number
  /** Concrete SDD stage that entered terminal `blocked`. */
  blocked_phase?: SpecPhase | null
  /** Latest role-gate verdict, retained across reconnect/reload. */
  gate_summary?: string | null
  execution_mode?: 'sequential' | 'orchestrated'
  max_concurrency?: number
  coordinator_session?: string | null
  active_workers?: HarnessWorkerStatus[]
}

type HarnessFiles = {
  agents_md?: string | null
  feature_list_json?: string | null
  init_sh?: string | null
  verify_sh?: string | null
  handoff_md?: string | null
}

// `HarnessEvent` is a serde-tagged enum: `{ "type": "...", ...fields }`.
export type HarnessEvent =
  | { type: 'state_changed'; harness_id: string; state: HarnessState }
  | { type: 'feature_state_changed'; harness_id: string; feature_id: string; state: FeatureState }
  | { type: 'init_started'; harness_id: string }
  | { type: 'init_completed'; harness_id: string; success: boolean; output: string }
  | { type: 'agent_spawned'; harness_id: string; feature_id: string; session_id: string }
  | {
      type: 'current_session_changed'
      harness_id: string
      session_id: string
      feature_id?: string | null
      agent_tool: string
    }
  | { type: 'log'; harness_id: string; feature_id?: string | null; message: string }
  | { type: 'verify_started'; harness_id: string; feature_id: string }
  | { type: 'verify_completed'; harness_id: string; feature_id: string; success: boolean; output: string }
  | { type: 'handoff_written'; harness_id: string; feature_id: string }
  | { type: 'harness_completed'; harness_id: string; success: boolean }
  | { type: 'phase_changed'; harness_id: string; from: SpecPhase; to: SpecPhase }
  | {
      type: 'gate_result'
      harness_id: string
      role: RoleKind
      passed: boolean
      attempt: number
      summary: string
    }
  | { type: 'worker_changed'; harness_id: string; task_id: string; session_id?: string | null; state: string }
  | { type: 'patch_changed'; harness_id: string; task_id: string; patch_id: string; state: string }
  | { type: 'ownership_conflict'; harness_id: string; task_id: string; path: string; message: string }
  | { type: 'task_verification'; harness_id: string; task_id: string; success: boolean }
  | { type: 'coordinator_rotated'; harness_id: string; previous_session: string; replacement_session: string }
  | { type: 'error'; harness_id: string; message: string }
  | { type: 'lagged'; skipped: number }

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
    throw new Error(`harness ${res.status} on ${path}${detail ? ` — ${detail}` : ''}`)
  }
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

export type HarnessWorktreeTarget = {
  workdir: string
  /** Authoritative identity; the server resolves host/path from it. */
  worktreeId: string
}

/** `POST /api/harness` — register a run from a worktree containing a Harness. */
export function startHarness(input: HarnessWorktreeTarget): Promise<{ harness_id: string }> {
  return request('/api/harness', {
    method: 'POST',
    body: JSON.stringify({
      workdir: input.workdir,
      worktreeId: input.worktreeId
    })
  })
}

/** Wire shape of `POST /api/harness/start-work` (spec 005 F1 — camelCase,
 *  matching the newer `SpecFromIssueResponse` precedent). */
export type StartGatedWorkResult = {
  harnessId: string
  worktreeId?: string
  repoId?: string
  hostId?: string
  executionMode?: 'sequential' | 'orchestrated'
  specId: string
  specExisted: boolean
  planned: number
  runStarted: boolean
  /** A live run already drives this worktree — a friendly state, not an error. */
  alreadyRunning: boolean
}

/**
 * `POST /api/harness/start-work` — the one-click issue → gated run
 * orchestration (spec 005 F1): converge-scaffold + plan from the linked issue,
 * initial Todo transition, agent/model knob write, then register + run the
 * Harness Engine against the worktree. Server-side so every caller (composer,
 * Tasks page) shares one failure surface.
 */
export function startGatedWork(input: {
  workdir: string
  /** Authoritative identity for local and SSH worktrees. */
  worktreeId: string
  number: number
  slug?: string
  agentTool?: string
  agentModel?: string
  /** Spec 021 (#379): the repo's tracker pin (`auto`/`github`/`linear`).
   *  Absent/`auto` keeps the issue-driven path's GitHub stamping. */
  tracker?: string
}): Promise<StartGatedWorkResult> {
  return request('/api/harness/start-work', {
    method: 'POST',
    body: JSON.stringify({
      workdir: input.workdir,
      worktreeId: input.worktreeId,
      number: String(input.number),
      ...(input.slug ? { slug: input.slug } : {}),
      ...(input.agentTool ? { agentTool: input.agentTool } : {}),
      ...(input.agentModel ? { agentModel: input.agentModel } : {}),
      ...(input.tracker ? { tracker: input.tracker } : {})
    })
  })
}

/** Call one of agentum's MCP tools over JSON-RPC at `POST /mcp`. Returns the
 *  tool's text payload. Used for the spec-010 surface tools that have no REST
 *  route (scaffold/plan/…). */
async function callMcpTool(
  name: string,
  args: Record<string, unknown>
): Promise<string> {
  const url = await apiUrl('/mcp')
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'tools/call',
      params: { name, arguments: args }
    })
  })
  if (!res.ok) throw new Error(`mcp ${res.status} on ${name}`)
  const j = (await res.json()) as {
    error?: { message?: string }
    result?: { content?: Array<{ text?: string }>; isError?: boolean }
  }
  if (j.error) throw new Error(j.error.message ?? `mcp error on ${name}`)
  const text = j.result?.content?.[0]?.text ?? ''
  if (j.result?.isError) throw new Error(text || `mcp tool ${name} failed`)
  return text
}

/** Scaffold the unified `.agentum-harness/` surface into `workdir` (spec 010a). */
function scaffoldHarness(workdir: string): Promise<string> {
  return callMcpTool('agentum_harness_scaffold', { workdir })
}

/** Full wire shape of `GET /api/harness/settings` (spec 005 F3 + 006 F3). */
export type HarnessSettings = {
  browserQaAgentEnabled: boolean
  /** Spec 006 F3: run the SDD role loop (PM → Architect → Build → Review
   *  gates) on start-work-planned backlogs. Server default ON. */
  sddRolesEnabled: boolean
}

/**
 * `GET /api/harness/settings` — engine-wide run behavior knobs: the browser-QA
 * capability switch (when on, the QA gate's `Auto` arm treats a spawned
 * `agentum_browser` QA agent as capable without `AGENTUM_BROWSER_VERIFY`;
 * default OFF, D3) and the SDD role-loop switch (default ON, spec 006 D1).
 */
export function getHarnessSettings(): Promise<HarnessSettings> {
  return request('/api/harness/settings')
}

/**
 * `PUT /api/harness/settings` — persist harness run-behavior knobs. PATCH
 * semantics (spec 006 C2): send only the keys you flip — two independent
 * Settings toggles can never clobber each other. Returns the full effective
 * settings.
 */
export function setHarnessSettings(patch: Partial<HarnessSettings>): Promise<HarnessSettings> {
  return request('/api/harness/settings', {
    method: 'PUT',
    body: JSON.stringify(patch)
  })
}

/** `GET /api/harness` — status for every registered run. */
export function listHarnesses(): Promise<HarnessStatus[]> {
  return request('/api/harness')
}

/** `GET /api/harness/{id}` — one run's status snapshot. */
export function getHarnessStatus(id: string): Promise<HarnessStatus> {
  return request(`/api/harness/${id}`)
}

/** `POST /api/harness/{id}/run` — kick off the end-to-end drive loop. */
export function runHarness(id: string): Promise<void> {
  return request(`/api/harness/${id}/run`, { method: 'POST' })
}

/** `POST /api/harness/{id}/init` — run init.sh only (manual env check). */
function initHarness(id: string): Promise<boolean> {
  return request(`/api/harness/${id}/init`, { method: 'POST' })
}

/** `POST /api/harness/{id}/verify` — run the gate for one feature (manual). */
function verifyFeature(id: string, featureId: string): Promise<boolean> {
  return request(`/api/harness/${id}/verify`, {
    method: 'POST',
    body: JSON.stringify({ feature_id: featureId })
  })
}

/** `GET /api/harness/{id}/files` — current `.harness/` file contents. */
function getHarnessFiles(id: string): Promise<HarnessFiles> {
  return request(`/api/harness/${id}/files`)
}

/** `DELETE /api/harness/{id}` — drop the run from the engine. */
function stopHarness(id: string): Promise<void> {
  return request(`/api/harness/${id}`, { method: 'DELETE' })
}

/**
 * `POST /api/harness/{id}/unlink-issue` — detach the run's tracker issue
 * (spec 023 Part B, AC 5) WITHOUT deleting the run: the server clears every
 * feature's `tracker_provider`/`tracker_url` and persists `feature_list.json`,
 * so later state transitions post nothing to the old issue (AC 6).
 */
export function unlinkHarnessIssue(id: string): Promise<void> {
  return request(`/api/harness/${id}/unlink-issue`, { method: 'POST' })
}

/** Handle for the live harness event stream. */
export type HarnessEventStream = { close: () => void }

/**
 * Open `WS /api/harness/events` and forward each parsed `HarnessEvent`. Auto-
 * reconnects with capped backoff (the bus is process-wide so a dropped socket
 * is always recoverable). The token rides in `?token=` because browsers can't
 * set headers on a WS upgrade.
 */
export async function openHarnessEventStream(
  onEvent: (ev: HarnessEvent) => void,
  onConnected?: () => void
): Promise<HarnessEventStream> {
  const { token } = await getServerEndpoint()
  const base = await wsUrl('/api/harness/events')
  const url = token ? `${base}?token=${encodeURIComponent(token)}` : base

  let ws: WebSocket | null = null
  let disposed = false
  let attempt = 0
  let timer: ReturnType<typeof setTimeout> | null = null

  const connect = (): void => {
    if (disposed) return
    const sock = new WebSocket(url)
    ws = sock
    sock.addEventListener('open', () => {
      attempt = 0
      onConnected?.()
    })
    sock.addEventListener('message', (event) => {
      if (typeof event.data !== 'string') return
      try {
        onEvent(JSON.parse(event.data) as HarnessEvent)
      } catch {
        // Ignore malformed frames rather than tearing the stream down.
      }
    })
    sock.addEventListener('close', () => {
      if (sock !== ws || disposed) return
      attempt += 1
      timer = setTimeout(connect, backoffMs(attempt))
    })
  }

  connect()

  return {
    close: () => {
      disposed = true
      if (timer) clearTimeout(timer)
      ws?.close()
    }
  }
}

/**
 * Subscribe to the FULL harness event stream — every event for every run
 * (spec 023 Part A). Thin exported wrap over the same auto-reconnecting
 * events WS: unlike `subscribeHarnessRunErrors` it never self-closes, so the
 * caller owns the lifecycle through the returned handle's `close()`.
 */
export async function subscribeHarnessEvents(
  onEvent: (ev: HarnessEvent) => void,
  onConnected?: () => void
): Promise<HarnessEventStream> {
  return openHarnessEventStream(onEvent, onConnected)
}

/**
 * Spec 008 F1 §B.5: surface a composer-started run's early drive-phase failure.
 * `start-work` returns fast (the drive loop is a background task) and the
 * composer navigates to the session view, so an `error` event on
 * `WS /api/harness/events` would otherwise have no one watching. This subscribes
 * to the event stream filtered by `harnessId` and invokes `onError` ONCE — on
 * the first `error` for that run — then self-closes. It also self-closes after
 * `windowMs` so a healthy run never holds the socket open indefinitely. Returns
 * a handle so the caller can cancel early. Best-effort: reuses the same
 * auto-reconnecting events-WS plumbing the Harness page uses.
 */
export async function subscribeHarnessRunErrors(
  harnessId: string,
  onError: (message: string) => void,
  windowMs = 120_000
): Promise<HarnessEventStream> {
  let fired = false
  let stream: HarnessEventStream | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  const dispose = (): void => {
    if (timer) {
      clearTimeout(timer)
      timer = null
    }
    stream?.close()
  }
  stream = await openHarnessEventStream((ev) => {
    if (fired) return
    if (ev.type === 'error' && ev.harness_id === harnessId) {
      fired = true
      onError(ev.message)
      dispose()
    }
  })
  timer = setTimeout(dispose, windowMs)
  return { close: dispose }
}
