import { parseOwnerRepoFromItemUrl } from '@/lib/github-item-url'
import React, { useMemo } from 'react'
import { useAppStore } from '@/store'
import IssueSourceIndicator, { sameGitHubOwnerRepo } from '@/components/github/IssueSourceIndicator'
import type { GitHubOwnerRepo } from '@/shared/types'
import { PER_REPO_FETCH_LIMIT } from '@/shared/work-items'

// Why: the dialog doesn't carry the resolved PR-source slug the Tasks view's
// list cache carries, so we reach into workItemsCache to recover it. We scope
// the lookup to the dialog's own `repoPath` via the public
// `getWorkItemsAnySourcesForRepo` selector keyed by (repoPath, limit) —
// scanning the whole cache risks picking a sibling repo's PR-source when two
// selected repos share the same issue-source (e.g. two forks of the same
// upstream), producing an incorrect "Issues from" chip or incorrectly
// suppressing it. The selector keys primarily on the first-page entry
// (PER_REPO_FETCH_LIMIT, empty query) because sources are repo-level and
// don't vary by search query. If that slot is empty — e.g. the Tasks view is
// filtering by a typed query and only populated the query-keyed entry — the
// selector falls back to scanning cache entries prefixed by this same
// `repoPath::` and reuses sources from the first match. Falling back to hiding
// the indicator when we still can't find a match matches the parent design
// doc §1 rule: hide when either side is unknown rather than guessing.
export function WorkItemIssueSourceIndicator({
  url,
  repoId
}: {
  url: string
  repoId: string | null
}): React.JSX.Element | null {
  // Why: subscribe to a single store-side selector that returns the resolved
  // sources for this repo — either the primary `(repoPath, PER_REPO_FETCH_LIMIT, '')`
  // entry or the first sibling cache entry that has sources (the Tasks view may
  // write cache entries keyed by a user-typed search query, so the primary slot
  // can be empty even when sources are known). Sources are repo-level
  // (query-independent), so any sibling entry is safe. When the primary slot
  // is populated its reference is stable across unrelated cache writes; when
  // the fallback path is used a sibling cache rewrite may produce a new
  // `sources` object and trigger a harmless extra render. That's cheap — the
  // indicator is small and the cache rewrite rate is bounded by user-initiated
  // refresh/search actions.
  const sources = useAppStore((s) =>
    s.getWorkItemsAnySourcesForRepo(repoId ?? '', PER_REPO_FETCH_LIMIT)
  )
  const issues = useMemo<GitHubOwnerRepo | null>(() => {
    const fromUrl = parseOwnerRepoFromItemUrl(url)
    if (!fromUrl) {
      return null
    }
    // Prefer the cache's resolved issue-source when it matches the URL-derived
    // slug — the cache entry is authoritative (canonicalized by the main
    // process) while the URL parse is a best-effort fallback.
    const cachedIssues = sources?.issues
    if (cachedIssues && sameGitHubOwnerRepo(cachedIssues, fromUrl)) {
      return cachedIssues
    }
    return fromUrl
  }, [url, sources])
  const prs = sources?.prs ?? null

  if (!issues || !prs || sameGitHubOwnerRepo(issues, prs)) {
    return null
  }
  return (
    <div className="mt-1">
      <IssueSourceIndicator issues={issues} prs={prs} variant="item" />
    </div>
  )
}
