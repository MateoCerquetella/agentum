// Pure orchestration model for the spec-018 issue hover-card Project-status
// chip (#365): parse an issue URL → its repo slug + number, then resolve the
// issue's GitHub Project Status option through two injected fetchers behind two
// caches (binding per slug, status per issue). Kept IO-free (fetchers + caches
// are injected) so it's trivially testable — mirrors lib/tracker-phase.ts.
//
// Contract: resolve() NEVER throws and NEVER rejects. Every miss, unbound repo,
// off-project issue, or fetch error resolves to `null` (spec 018 AC 2, silent
// absence). The caller renders a chip iff the result is a non-empty string.

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

/** The binding fields the status read needs. `null` = repo has no binding. */
export type ProjectBindingRef = {
  projectId: string
  statusFieldId: string
} | null

/** IO seam. Both fetchers may throw/reject — resolve() swallows that to null. */
export type IssueProjectStatusDeps = {
  /** App-session cache: slug → binding (`null` = confirmed unbound). */
  bindingCache: Map<string, ProjectBindingRef>
  /** App-session cache: `slug#number` → status option name (`null` = none). */
  statusCache: Map<string, string | null>
  /** Read the repo's Projects v2 binding (the existing getProjectBinding). */
  getBinding: (ref: IssueRef) => Promise<ProjectBindingRef>
  /** Read the issue's Status option on the bound project (the new command). */
  getStatus: (ref: IssueRef, binding: NonNullable<ProjectBindingRef>) => Promise<string | null>
}

/** Resolve the issue's Project Status option name, or null. Fills both caches;
 *  a second call for the same issue hits the caches and issues no fetch. Never
 *  throws — every failure path returns null. */
export async function resolveIssueProjectStatus(
  ref: IssueRef,
  deps: IssueProjectStatusDeps
): Promise<string | null> {
  const key = statusCacheKey(ref.slug, ref.number)
  if (deps.statusCache.has(key)) {
    return deps.statusCache.get(key) ?? null
  }

  let binding: ProjectBindingRef
  if (deps.bindingCache.has(ref.slug)) {
    binding = deps.bindingCache.get(ref.slug) ?? null
  } else {
    try {
      binding = await deps.getBinding(ref)
    } catch {
      binding = null
    }
    deps.bindingCache.set(ref.slug, binding)
  }

  if (!binding) {
    // Unbound repo: cache the miss so we never refetch the status either.
    deps.statusCache.set(key, null)
    return null
  }

  let status: string | null
  try {
    status = await deps.getStatus(ref, binding)
  } catch {
    status = null
  }
  status = normalizeStatus(status)
  deps.statusCache.set(key, status)
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
