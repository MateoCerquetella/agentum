// Typed client for the board routes on the embedded agentum-server
// (`/api/board*`). Mirrors `harness-client.ts`: same loopback endpoint +
// auth, wire shapes faithful to `crates/agentum-server/src/routes/board.rs`
// and `agentum_core::BoardItem` (serde — snake_case field names) so there is
// one source of truth and no silent field drift.
//
// #254: the internal-board card UI (goals/cards kanban with its own start
// control) is gone — Chat files to GitHub/Linear and the Tasks board spawns
// sessions through the workspace composer. The dead spawn plumbing
// (`startCard`/`moveCard` and friends) was removed with it; only the external
// issue sync remains in use.
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

/**
 * `POST /api/board/sync` — mirror external tracker issues onto the internal
 * board (idempotent by `external_url`: re-syncing the same issues updates
 * the same cards instead of duplicating them). Returns the synced cards.
 */
export function syncExternalIssues(items: SyncIssueInput[]): Promise<{ synced: BoardItem[] }> {
  return request('/api/board/sync', { method: 'POST', body: JSON.stringify({ items }) })
}
