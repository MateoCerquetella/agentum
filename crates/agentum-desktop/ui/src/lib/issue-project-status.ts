// Pure orchestration model for the spec-018 issue hover-card Project-status
// chip (#365): parse an issue URL → its repo slug + number, then resolve the
// issue's GitHub Project Status option through two injected fetchers behind two
// caches (binding per slug, status per issue). Kept IO-free (fetchers, caches,
// and the clock are injected) so it's trivially testable — mirrors
// lib/tracker-phase.ts.
//
// Contract: resolve() NEVER throws and NEVER rejects. GitHub's returned option
// name is the only display value. Failures retain the last externally confirmed
// value and return an actionable warning; they never synthesize a desired local
// phase (issue #399).
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
export type IssueProjectStatusResult = {
  status: string | null
  statusOptionId: string | null
  warning: string | null
}

export type StatusCacheEntry = IssueProjectStatusResult & { fetchedAt: number }

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
  /** Read the issue's live Status option on the bound project. */
  getStatus: (
    ref: IssueRef,
    binding: NonNullable<ProjectBindingRef>
  ) => Promise<{ status: string | null; statusOptionId: string | null }>
  /** Clock seam for staleness (default Date.now). */
  now?: () => number
  /** Override for tests (default STATUS_STALE_AFTER_MS). */
  staleAfterMs?: number
}

/** Resolve GitHub's Project Status result. Fills both caches; a call inside the
 *  freshness window hits the caches unless the linked issue has just loaded
 *  with `forceRefresh`. Never throws: failures retain the last externally
 *  confirmed value and carry an actionable warning. */
export async function resolveIssueProjectStatus(
  ref: IssueRef,
  deps: IssueProjectStatusDeps,
  options: { forceRefresh?: boolean } = {}
): Promise<IssueProjectStatusResult> {
  const now = deps.now ?? Date.now
  const staleAfterMs = deps.staleAfterMs ?? STATUS_STALE_AFTER_MS
  const key = statusCacheKey(ref.slug, ref.number)

  const cached = deps.statusCache.get(key)
  if (!options.forceRefresh && cached && now() - cached.fetchedAt < staleAfterMs) {
    return cached
  }

  // A stored binding is reused for the session (project/field ids are stable);
  // a stored `null` (unbound) is re-probed so a later binding is picked up.
  let binding = deps.bindingCache.get(ref.slug) ?? null
  if (!binding) {
    try {
      binding = await deps.getBinding(ref)
    } catch (error) {
      return resultWithWarning(cached, error)
    }
    deps.bindingCache.set(ref.slug, binding)
  }

  if (!binding) {
    const result = { status: null, statusOptionId: null, warning: null }
    deps.statusCache.set(key, { ...result, fetchedAt: now() })
    return result
  }

  try {
    const external = await deps.getStatus(ref, binding)
    const result = {
      status: normalizeStatus(external.status),
      statusOptionId: normalizeStatus(external.statusOptionId),
      warning: null
    }
    deps.statusCache.set(key, { ...result, fetchedAt: now() })
    return result
  } catch (error) {
    const result = resultWithWarning(cached, error)
    deps.statusCache.set(key, { ...result, fetchedAt: now() })
    return result
  }
}

function resultWithWarning(
  cached: StatusCacheEntry | undefined,
  error: unknown
): IssueProjectStatusResult {
  const detail =
    error instanceof Error && error.message.trim() ? error.message.trim() : 'unknown error'
  return {
    status: cached?.status ?? null,
    statusOptionId: cached?.statusOptionId ?? null,
    warning: `GitHub status sync pending: ${detail}. Check gh authentication and the Project binding; Agentum will retry.`
  }
}

/** A blank/whitespace-only option name is "no status", not an empty chip. */
function normalizeStatus(status: string | null): string | null {
  if (typeof status !== 'string') {
    return null
  }
  const trimmed = status.trim()
  return trimmed.length > 0 ? trimmed : null
}
