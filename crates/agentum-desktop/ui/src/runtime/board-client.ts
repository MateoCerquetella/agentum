// Typed client for the board routes on the embedded agentum-server
// (`/api/board*`). Mirrors `harness-client.ts`: same loopback endpoint +
// auth, wire shapes faithful to `crates/agentum-server/src/routes/board.rs`
// and `agentum_core::BoardItem` (serde — snake_case field names) so there is
// one source of truth and no silent field drift.
import { apiUrl, getServerEndpoint } from './server-endpoint'

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
}

/** `GET /api/board` response — items grouped by status column. */
export type GroupedBoard = {
  columns: Record<string, BoardItem[]>
  column_order: string[]
  comment_counts: Record<number, number>
}

/** `POST /api/board/goals` response. */
export type CreateGoalResult = {
  goal: BoardItem
  planner_session_id: string
}

/** A goal together with the feature cards the planner produced under it. */
export type GoalWithChildren = {
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
export function listBoard(): Promise<GroupedBoard> {
  return request('/api/board')
}

/**
 * `POST /api/board/goals` — create a goal from a natural-language description.
 * The server's planner then decomposes it into child feature cards
 * asynchronously, so callers should refresh the board afterwards.
 */
export function createGoal(input: {
  title: string
  body?: string
  workdir?: string
}): Promise<CreateGoalResult> {
  return request('/api/board/goals', { method: 'POST', body: JSON.stringify(input) })
}

/**
 * `PATCH /api/board/{id}` with a new status. Moving a card to `"doing"`
 * triggers the server's card-start path (`spawn_card_session`): it provisions
 * a per-card worktree and spawns the agent into it, returning the card with
 * its now-bound `session_id`. Other status moves just update the column.
 */
export function moveCard(id: number, status: string): Promise<BoardItem> {
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
export function startCard(id: number): Promise<BoardItem> {
  return moveCard(id, 'doing')
}

/**
 * Flatten a grouped board into goals (`lbl: 'goal'`) each paired with their
 * child cards (`parent_goal_id === goal.id`). Pure — no IO — so it's trivial
 * to unit-test and to reuse across renders.
 */
export function selectGoalsWithChildren(board: GroupedBoard): GoalWithChildren[] {
  const all = Object.values(board.columns).flat()
  const goals = all.filter((i) => i.lbl === 'goal')
  return goals.map((goal) => ({
    goal,
    children: all.filter((i) => i.parent_goal_id === goal.id)
  }))
}
