// Spec 358b: the hover card's fetch-once cache for an issue's Project Status.
//
// GitHub rate limits are the constraint: the status is fetched lazily on the
// FIRST hover of an issue and cached for the app session — a second hover
// (or a card remount) never refetches. Errors resolve — and cache — as `null`
// (silent absence): a tracker hiccup must never break the hover card, and a
// failed probe retrying on every hover would defeat the rate-limit budget.
// Pure of any component so the model is unit-testable; the chip component
// wires the real runtime client in as `fetcher`.

export type IssueProjectStatusRequest = {
  workdir: string
  number: number
  /** Resolve the repo's slug on its own host (SSH repos) — spec 020's wire. */
  repoId?: string
}

export type IssueProjectStatusFetcher = (
  input: IssueProjectStatusRequest
) => Promise<{ status: string | null }>

/** Keyed by repo identity + issue number. `repoId` wins when present (stable
 *  across path spellings); the workdir is the pre-020 local fallback. */
export function issueProjectStatusCacheKey(input: IssueProjectStatusRequest): string {
  return `${input.repoId ?? input.workdir}::#${input.number}`
}

// In-flight promises and settled values share one map: a concurrent second
// hover joins the pending fetch (single-flight) instead of doubling it.
const cache = new Map<string, Promise<string | null>>()

/** The issue's Status option name on the bound project, or `null` (absent).
 *  One network fetch per issue per app session, errors included. */
export function getCachedIssueProjectStatus(
  input: IssueProjectStatusRequest,
  fetcher: IssueProjectStatusFetcher
): Promise<string | null> {
  const key = issueProjectStatusCacheKey(input)
  const cached = cache.get(key)
  if (cached) {
    return cached
  }
  const pending = fetcher(input).then(
    (res) => res.status,
    () => null
  )
  cache.set(key, pending)
  return pending
}

/** Test seam — the cache is module-lifetime (the app session) by design. */
export function resetIssueProjectStatusCache(): void {
  cache.clear()
}
