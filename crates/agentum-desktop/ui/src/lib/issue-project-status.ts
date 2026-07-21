// Pure orchestration model for the spec-018 issue hover-card Project-status
// chip (#365): parse an issue URL → its repo slug + number, then resolve the
// issue's GitHub Project Status option through two injected fetchers behind two
// caches (binding per slug, status per issue). Kept IO-free (fetchers, caches,
// and the clock are injected) so it's trivially testable — mirrors
// lib/tracker-phase.ts.
//
// Contract: resolve() NEVER throws and NEVER rejects. Every miss, unbound repo,
// off-project issue, or fetch error resolves to `null` (spec 018 AC 2, silent
// absence). The caller renders a chip iff the result is a non-empty string.
//
// Freshness: the Status option is VOLATILE — the board column moves while a run
// is coding (Backlog → Todo → In Progress → …), so a session-long cache shows
// yesterday's column (#379 regression). Status entries carry a timestamp and go
// stale after STATUS_STALE_AFTER_MS: a stale hit refetches and, on a transient
// fetch error, falls back to the last-known value rather than blanking the
// chip. Rapid consecutive hovers inside the window still cost zero fetches
// (spec 018 AC 3's intent). A bound repo's binding ids are stable and stay
// cached for the session; an UNBOUND verdict is re-probed on revalidation so
// binding the repo later doesn't require an app restart.

/** Owner/repo/number distilled from a GitHub issue URL, plus the `owner/repo`
 *  slug used to key the binding cache and the binding lookup. */
export type IssueRef = {
  owner: string
  repo: string
  number: number
  slug: string
}

/** Parse `https://github.com/<owner>/<repo>/issues/<n>`; anything else → null
 *  (a non-GitHub URL, a PR URL, a missing URL) so the chip stays absent. */
export function parseIssueRef(url: string | undefined | null): IssueRef | null {
  if (!url) {
    return null
  }
  const match = /^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/issues\/(\d+)(?:[/?#].*)?$/.exec(
    url.trim()
  )
  if (!match) {
    return null
  }
  const [, owner, repo, numberStr] = match
  const number = Number.parseInt(numberStr, 10)
  if (!Number.isSafeInteger(number) || number <= 0) {
    return null
  }
  return { owner, repo, number, slug: `${owner}/${repo}` }
}

/** Status cache key — one issue on its repo. */
export function statusCacheKey(slug: string, number: number): string {
  return `${slug}#${number}`
}

/** How long a fetched Status option is trusted before the next hover
 *  revalidates it. */
export const STATUS_STALE_AFTER_MS = 30_000

/** A cached status read: the option name (`null` = none/unbound) + when it was
 *  fetched, so hovers after STATUS_STALE_AFTER_MS revalidate. */
export type StatusCacheEntry = { status: string | null; fetchedAt: number }

/** The binding fields the status read needs. `null` = repo has no binding. */
export type ProjectBindingRef = {
  projectId: string
  statusFieldId: string
} | null

/** IO seam. Both fetchers may throw/reject — resolve() swallows that to null. */
export type IssueProjectStatusDeps = {
  /** App-session cache: slug → binding (`null` = unbound at last probe). */
  bindingCache: Map<string, ProjectBindingRef>
  /** App-session cache: `slug#number` → timestamped status. */
  statusCache: Map<string, StatusCacheEntry>
  /** Read the repo's Projects v2 binding (the existing getProjectBinding). */
  getBinding: (ref: IssueRef) => Promise<ProjectBindingRef>
  /** Read the issue's Status option on the bound project (the new command). */
  getStatus: (ref: IssueRef, binding: NonNullable<ProjectBindingRef>) => Promise<string | null>
  /** Clock seam for staleness (default Date.now). */
  now?: () => number
  /** Override for tests (default STATUS_STALE_AFTER_MS). */
  staleAfterMs?: number
}

/** Resolve the issue's Project Status option name, or null. Fills both caches;
 *  a call inside the freshness window hits the caches and issues no fetch, a
 *  later one revalidates. Never throws — every failure path returns null (or
 *  the last-known status on a revalidation error). */
export async function resolveIssueProjectStatus(
  ref: IssueRef,
  deps: IssueProjectStatusDeps
): Promise<string | null> {
  const now = deps.now ?? Date.now
  const staleAfterMs = deps.staleAfterMs ?? STATUS_STALE_AFTER_MS
  const key = statusCacheKey(ref.slug, ref.number)

  const cached = deps.statusCache.get(key)
  if (cached && now() - cached.fetchedAt < staleAfterMs) {
    return cached.status
  }

  // A stored binding is reused for the session (project/field ids are stable);
  // a stored `null` (unbound) is re-probed so a later binding is picked up.
  let binding = deps.bindingCache.get(ref.slug) ?? null
  if (!binding) {
    try {
      binding = await deps.getBinding(ref)
    } catch {
      binding = null
    }
    deps.bindingCache.set(ref.slug, binding)
  }

  if (!binding) {
    deps.statusCache.set(key, { status: null, fetchedAt: now() })
    return null
  }

  let status: string | null
  try {
    status = normalizeStatus(await deps.getStatus(ref, binding))
  } catch {
    // Transient revalidation failure: keep showing the last-known column
    // instead of blanking an already-rendered chip; a first fetch stays null.
    status = cached ? cached.status : null
  }
  deps.statusCache.set(key, { status, fetchedAt: now() })
  return status
}

/** A blank/whitespace-only option name is "no status", not an empty chip. */
function normalizeStatus(status: string | null): string | null {
  if (typeof status !== 'string') {
    return null
  }
  const trimmed = status.trim()
  return trimmed.length > 0 ? trimmed : null
}
