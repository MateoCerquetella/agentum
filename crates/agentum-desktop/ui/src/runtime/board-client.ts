// Typed client for the board routes on the embedded agentum-server
// (`/api/board*`). Mirrors `harness-client.ts`: same loopback endpoint +
// auth, wire shapes faithful to `crates/agentum-server/src/routes/board.rs`
// and `agentum_core::BoardItem` (serde — snake_case field names) so there is
// one source of truth and no silent field drift.
import { apiUrl, getServerEndpoint } from './server-endpoint'
import { subscribeServerEvents } from './server-events-bus'

/** A board ticket — `agentum_core::BoardItem`. Goals are items with `lbl: 'goal'`. */
export type BoardItem = {
  id: number
  /** Human-friendly key, e.g. `AG-7`. */
  key: string
  title: string
  body?: string | null
  status: string
  claimed_by?: string | null
  created_at: string
  updated_at: string
  lbl?: string | null
  tool?: string | null
  workdir?: string | null
  model?: string | null
  session_id?: string | null
  /** Set on child cards; points at the parent goal's `id`. */
  parent_goal_id?: number | null
  priority: number
  /** Web URL of the external issue this card mirrors (GitHub/Linear), when the
   *  card came from a tracker sync. Absent for native agentum cards. */
  external_url?: string | null
  /** `github` | `linear` | `gitlab` — drives the source badge + open link. */
  external_provider?: string | null
}

/** One external issue to fold onto the board via `POST /api/board/sync`. */
export type SyncIssueInput = {
  external_url: string
  external_provider?: string
  title: string
  body?: string
  /** Board column (todo/doing/review/done). Defaults to todo server-side. */
  status?: string
  /** Source label for the card foot badge, e.g. 'github' / 'linear'. */
  lbl?: string
}

/** `GET /api/board` response — items grouped by status column. */
type GroupedBoard = {
  columns: Record<string, BoardItem[]>
  column_order: string[]
  comment_counts: Record<number, number>
}

/** Where a created feature landed — `crate::task_sink::FeatureRef` (serde). The
 *  GitHub issue / Linear ticket / board card the server created on Chat submit
 *  (spec 018). `url` is absent for board cards (and may be for Linear). */
type FeatureRef = {
  /** `github` | `linear` | `board` — drives the link + the "created on …" copy. */
  provider: string
  /** Provider-stable handle: a GitHub issue number, a Linear id, or a board key. */
  id: string
  url?: string | null
}

/** `POST /api/board/goals` response (spec 018) — the goal tracking row plus the
 *  created feature (issue/card). Replaces the old `planner_session_id`. */
type CreateGoalResult = {
  goal: BoardItem
  feature: FeatureRef
}

/** The server's typed AC-3 error body: `{ error: { code, message, provider } }`.
 *  Thrown by `createGoal` so the Chat UI can show the *specific* reason (e.g.
 *  "Connect GitHub / not a GitHub repo") and branch on `code` — never a silent
 *  indefinite "planning…". */
class CreateGoalError extends Error {
  readonly code: string
  readonly provider: string | null
  readonly status: number
  constructor(args: { code: string; message: string; provider: string | null; status: number }) {
    super(args.message)
    this.name = 'CreateGoalError'
    this.code = args.code
    this.provider = args.provider
    this.status = args.status
  }
}

/** A goal together with the feature cards the planner produced under it. */
type GoalWithChildren = {
  goal: BoardItem
  children: BoardItem[]
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
    throw new Error(`board ${res.status} on ${path}${detail ? ` — ${detail}` : ''}`)
  }
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

/** `GET /api/board` — the whole board, grouped by status column. */
function listBoard(): Promise<GroupedBoard> {
  return request('/api/board')
}

/**
 * `POST /api/board/goals` — create a goal from a natural-language description and
 * (spec 018) **deterministically** create one tracker item for it server-side
 * (GitHub issue / Linear ticket / board card). Resolves to the created
 * {@link FeatureRef}; on failure throws a {@link CreateGoalError} carrying the
 * server's typed `{ code, message, provider }` so the caller shows the specific
 * reason. No agent is spawned — the promise settles when the issue exists or a
 * loud error is known (no indefinite "planning…").
 *
 * Has its own fetch path (not the string-flattening `request`) precisely so the
 * AC-3 error envelope survives as structured fields.
 */
async function createGoal(input: {
  title: string
  body?: string
  workdir?: string
  /** Which agent the goal's child cards inherit when started from the board,
   *  e.g. "claude" | "codex" | "gemini". No longer drives a planner. */
  tool?: string
  model?: string
  /** SSH host the `workdir` lives on (S3 / AC-6). Omitted = local repo. */
  host_id?: string | null
  /** Optional `owner/repo` hint for the GitHub issue target (spec 019). When
   *  present the server skips its host-aware `origin` read; when absent the
   *  server resolves the slug authoritatively from the project's remote. */
  repo_slug?: string
}): Promise<CreateGoalResult> {
  const url = await apiUrl('/api/board/goals')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders())
    },
    body: JSON.stringify(input)
  })
  const text = await res.text()
  if (res.ok) {
    return (text ? JSON.parse(text) : undefined) as CreateGoalResult
  }
  // Parse the typed envelope `{ error: { code, message, provider } }`. Fall back
  // to the raw body / status when the shape is unexpected (older server, proxy
  // error) so the user still sees *something* specific, never a silent hang.
  let code = 'error'
  let message = `board ${res.status}`
  let provider: string | null = null
  try {
    const parsed = JSON.parse(text) as {
      error?: { code?: string; message?: string; provider?: string | null } | string
    }
    if (parsed.error && typeof parsed.error === 'object') {
      code = parsed.error.code ?? code
      message = parsed.error.message ?? message
      provider = parsed.error.provider ?? null
    } else if (typeof parsed.error === 'string') {
      message = parsed.error
    }
  } catch {
    if (text) message = text
  }
  throw new CreateGoalError({ code, message, provider, status: res.status })
}

/**
 * `PATCH /api/board/{id}` with a new status. Moving a card to `"doing"`
 * triggers the server's card-start path (`spawn_card_session`): it provisions
 * a per-card worktree and spawns the agent into it, returning the card with
 * its now-bound `session_id`. Other status moves just update the column.
 */
function moveCard(id: number, status: string): Promise<BoardItem> {
  return request(`/api/board/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ status })
  })
}

/**
 * Start a card: move it to `"doing"`, which spawns its agent (per-card
 * worktree + shared launch path). Returns the card with `session_id` set so
 * the caller can immediately open the live agent workspace.
 */
function startCard(id: number): Promise<BoardItem> {
  return moveCard(id, 'doing')
}

/**
 * `DELETE /api/board/{id}` — remove a single board item. The server does NOT
 * cascade to child cards (it deletes one row), so deleting a goal that still
 * has children would orphan them; callers that want the whole chat gone must
 * delete the children first (see `deleteGoalWithChildren`).
 */
function deleteBoardItem(id: number): Promise<void> {
  return request(`/api/board/${id}`, { method: 'DELETE' })
}

/**
 * Delete a goal/chat and all the cards the planner drafted under it, children
 * first so no card is left pointing at a missing parent. Used by Chat's
 * "delete chat" action (and to clear orphaned "planning…" goals).
 */
async function deleteGoalWithChildren(
  goal: GoalWithChildren
): Promise<void> {
  for (const child of goal.children) {
    await deleteBoardItem(child.id)
  }
  await deleteBoardItem(goal.goal.id)
}

/**
 * `POST /api/board/sync` — fold GitHub/Linear issues onto the board (#48).
 * Idempotently upserts each issue as a card keyed on `external_url`, so the
 * Tasks view becomes a sync source feeding the one board (re-syncing updates
 * the same cards instead of duplicating them). Returns the synced cards.
 */
export function syncExternalIssues(items: SyncIssueInput[]): Promise<{ synced: BoardItem[] }> {
  return request('/api/board/sync', { method: 'POST', body: JSON.stringify({ items }) })
}

/** Handle for the live board event subscription. */
type BoardEventStream = { close: () => void }

/**
 * The global-bus event `kind`s that move a card between columns: a card-start
 * spawn (`board.updated`) and the agent lifecycle the server's task_sink maps
 * onto card status (started → Building, awaiting/finished/crashed → Review/Done).
 * Any of these means the board may have changed, so the caller should refresh.
 */
const BOARD_RELEVANT_KINDS = new Set([
  'board.updated',
  'session.started',
  'agent.awaiting_input',
  'agent.finished',
  'session.crashed'
])

/**
 * Subscribe to the global event bus (`WS /api/events`) and invoke `onChange`
 * whenever a board-relevant lifecycle event arrives — so columns transition
 * live instead of waiting for the next poll. Rides the SHARED events socket
 * (server-events-bus): one connection + one parse per frame app-wide, with
 * reconnect/backoff owned by the bus. Kept async for caller compatibility.
 */
async function openBoardEventStream(onChange: () => void): Promise<BoardEventStream> {
  const unsubscribe = subscribeServerEvents({
    onEvent: (ev) => {
      if (typeof ev.kind === 'string' && BOARD_RELEVANT_KINDS.has(ev.kind)) {
        onChange()
      }
    }
  })
  return { close: unsubscribe }
}

/**
 * Flatten a grouped board into goals (`lbl: 'goal'`) each paired with their
 * child cards (`parent_goal_id === goal.id`). Pure — no IO — so it's trivial
 * to unit-test and to reuse across renders.
 */
function selectGoalsWithChildren(board: GroupedBoard): GoalWithChildren[] {
  const all = Object.values(board.columns).flat()
  const goals = all.filter((i) => i.lbl === 'goal')
  return goals.map((goal) => ({
    goal,
    children: all.filter((i) => i.parent_goal_id === goal.id)
  }))
}
