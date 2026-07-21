// Why: the sidebar host order is a renderer-only presentation preference — the
// same class of "purely cosmetic, per-device" state as column widths
// (see components/github-project/column-widths.ts). It is persisted in
// localStorage, never the settings/store, so a reorder is a pure client-side
// action with no backend round-trip (spec 383 non-goal: no backend change).
//
// The order is stored as an ordered array of SSH host keys (`ssh:<connId>`).
// A sparse-rank scheme (like worktree-manual-order-ranks.ts) exists to survive
// concurrent backend writes from multiple clients — a problem we don't have for
// a device-local array — so the minimal fit is a plain sequence.
import { LOCAL_HOST_KEY } from './worktree-list-groups'

const STORAGE_KEY = 'agentum.sidebar.hostOrder'

/** Resolve the browser storage, tolerating non-DOM contexts (SSR / tests that
 *  don't stub `window`). Callers may inject a Storage explicitly. */
function defaultStorage(): Storage | null {
  try {
    return typeof window !== 'undefined' ? window.localStorage : null
  } catch {
    // Accessing localStorage can throw (disabled / sandboxed) — treat as absent.
    return null
  }
}

/** Read the persisted host order. Returns `[]` when absent or garbled so the
 *  caller falls back to the default (local-first, first-seen) ordering. */
export function loadHostOrder(storage: Storage | null = defaultStorage()): string[] {
  if (!storage) {
    return []
  }
  try {
    const raw = storage.getItem(STORAGE_KEY)
    if (!raw) {
      return []
    }
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) {
      return []
    }
    return parsed.filter((value): value is string => typeof value === 'string')
  } catch {
    return []
  }
}

/** Persist the host order. localStorage-only — never a network write. */
export function saveHostOrder(
  keys: readonly string[],
  storage: Storage | null = defaultStorage()
): void {
  if (!storage) {
    return
  }
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(keys))
  } catch {
    // localStorage may be disabled — the order just won't persist this session.
  }
}

/**
 * Resolve the effective host order from the currently-present host keys and the
 * persisted preference. Contract:
 *   - the local host is always pinned first (it is excluded from reordering),
 *   - persisted SSH keys are applied in their stored order,
 *   - hosts present but not covered by the persisted order (a newly added host,
 *     or first run) are appended after, in their first-seen `currentKeys` order,
 *   - persisted keys no longer present (stale / removed host) are dropped,
 *   - duplicates are removed.
 *
 * `currentKeys` is the first-seen host order (e.g. `['local','ssh:a','ssh:b']`);
 * `persisted` is the saved SSH order (e.g. `['ssh:b','ssh:a']`).
 */
export function applyPersistedHostOrder(
  currentKeys: readonly string[],
  persisted: readonly string[]
): string[] {
  const currentSet = new Set(currentKeys)
  const placed = new Set<string>()
  const result: string[] = []

  // 1. Local host is pinned first whenever present, regardless of the persisted
  //    array (a stale persisted `'local'` can never move it or duplicate it).
  if (currentSet.has(LOCAL_HOST_KEY)) {
    result.push(LOCAL_HOST_KEY)
    placed.add(LOCAL_HOST_KEY)
  }

  // 2. Apply the persisted SSH order — only keys that still exist, de-duped, and
  //    never the local host (pinned above, never part of the reorderable set).
  for (const key of persisted) {
    if (key === LOCAL_HOST_KEY || placed.has(key) || !currentSet.has(key)) {
      continue
    }
    result.push(key)
    placed.add(key)
  }

  // 3. Append any present host the persisted order didn't cover (newly added
  //    host, first run) in first-seen order — appears after the ordered hosts.
  for (const key of currentKeys) {
    if (placed.has(key)) {
      continue
    }
    result.push(key)
    placed.add(key)
  }

  return result
}
