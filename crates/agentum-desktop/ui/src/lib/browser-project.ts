import { splitWorktreeId } from '@/shared/worktree-id'

// Project identity for browser surfaces (spec 014): the UI mirror of the
// server's BrowserScope resolution — only the steps the UI can decide without
// registry access (`<repoId>::…` prefix, bare repo UUID). Everything else is
// null: those contexts have no project, so project-scoped browser actions
// (e.g. "Clear browsing data for this project") must not be offered on them.

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/**
 * The repo id (spec 014 D2: the project identity) behind a browser surface's
 * worktree id, or `null` when the context has no project — synthetic ids
 * (`global-floating-terminal`, `__orphan__`), `github-pr:*` pseudo-keys, bare
 * paths, empty. A `<repoId>::<path>` id (folder projects append
 * `::workspace:<uuid>`) yields the prefix; a bare repo UUID yields itself.
 */
export function deriveProjectRepoId(worktreeId: string | null | undefined): string | null {
  const raw = (worktreeId ?? '').trim()
  if (!raw) {
    return null
  }
  const parsed = splitWorktreeId(raw)
  if (parsed) {
    return parsed.repoId || null
  }
  return UUID_RE.test(raw) ? raw : null
}
