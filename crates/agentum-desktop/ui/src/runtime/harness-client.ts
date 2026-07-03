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

export type HarnessState =
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
export type RoleKind = 'pm' | 'architect' | 'reviewer'

export type Feature = {
  id: string
  name: string
  description: string
  state: FeatureState
  attempts: number
  last_error?: string | null
  prompt?: string | null
  /** Task tracker this feature mirrors (`board` / `github` / `linear`). */
  tracker_provider?: string | null
  /** The tracker item's URL (null for the internal board). */
  tracker_url?: string | null
}

export type FeatureList = {
  features: Feature[]
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
}

export type HarnessStatus = {
  id: string
  workdir: string
  state: HarnessState
  features: FeatureList
  current_feature?: string | null
  current_session?: string | null
  elapsed_secs: number
  agent_instructions: string
  /** Current SDD phase (spec 013); `executing` for a plain feature run. */
  phase?: SpecPhase
  /** Role-gate retry counter for the current phase (spec 013). */
  phase_attempts?: number
}

export type HarnessFiles = {
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

/** `POST /api/harness` — register a run from a project dir containing `.harness/`. */
export function startHarness(workdir: string): Promise<{ harness_id: string }> {
  return request('/api/harness', { method: 'POST', body: JSON.stringify({ workdir }) })
}

/** Wire shape of `POST /api/harness/start-work` (spec 005 F1 — camelCase,
 *  matching the newer `SpecFromIssueResponse` precedent). */
export type StartGatedWorkResult = {
  harnessId: string
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
  number: number
  slug?: string
  agentTool?: string
  agentModel?: string
}): Promise<StartGatedWorkResult> {
  return request('/api/harness/start-work', {
    method: 'POST',
    body: JSON.stringify({
      workdir: input.workdir,
      number: String(input.number),
      ...(input.slug ? { slug: input.slug } : {}),
      ...(input.agentTool ? { agentTool: input.agentTool } : {}),
      ...(input.agentModel ? { agentModel: input.agentModel } : {})
    })
  })
}

export type PlanGoalHarnessResult = {
  /** Which task manager backed the features: "board" | "github" | "linear". */
  provider: string
  workdir: string
  feature_count: number
  features: FeatureList
}

/**
 * `POST /api/board/goals/{id}/harness-plan` — turn a goal's planner-produced
 * child cards into the harness backlog (spec 011 chat-to-features). Writes
 * `feature_list.json` and leaves the harness **Idle**; the user reviews the
 * board and then runs it (human-gated). When an external task manager is
 * configured (GitHub/Linear) the cards are mirrored there and `provider`
 * reflects it; otherwise the internal board is the source of truth.
 */
export function planGoalHarness(goalId: number): Promise<PlanGoalHarnessResult> {
  return request(`/api/board/goals/${goalId}/harness-plan`, { method: 'POST' })
}

/** Call one of agentum's MCP tools over JSON-RPC at `POST /mcp`. Returns the
 *  tool's text payload. Used for the spec-010 surface tools that have no REST
 *  route (scaffold/plan/board/…). */
export async function callMcpTool(
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
export function scaffoldHarness(workdir: string): Promise<string> {
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
export function initHarness(id: string): Promise<boolean> {
  return request(`/api/harness/${id}/init`, { method: 'POST' })
}

/** `POST /api/harness/{id}/verify` — run the gate for one feature (manual). */
export function verifyFeature(id: string, featureId: string): Promise<boolean> {
  return request(`/api/harness/${id}/verify`, {
    method: 'POST',
    body: JSON.stringify({ feature_id: featureId })
  })
}

/** `GET /api/harness/{id}/files` — current `.harness/` file contents. */
export function getHarnessFiles(id: string): Promise<HarnessFiles> {
  return request(`/api/harness/${id}/files`)
}

/** `DELETE /api/harness/{id}` — drop the run from the engine. */
export function stopHarness(id: string): Promise<void> {
  return request(`/api/harness/${id}`, { method: 'DELETE' })
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
  onEvent: (ev: HarnessEvent) => void
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
