import { sameGitHubOwnerRepo } from '@/components/github/IssueSourceIndicator'
import { type TaskPageRepoSourceState } from '@/components/task-page-cache-selectors'
import type { GitHubOwnerRepo } from '@/shared/types'

// Why: type-guard predicate used to filter `perRepoSourceState` down to rows
// whose issue-source and PR-source slugs differ. Hoisted to module scope so
// the predicate isn't re-allocated on every TaskPage render.
export const hasDivergentSources = (
  s: TaskPageRepoSourceState
): s is TaskPageRepoSourceState & {
  sources: { issues: GitHubOwnerRepo; prs: GitHubOwnerRepo }
} => !!s.sources?.issues && !!s.sources.prs && !sameGitHubOwnerRepo(s.sources.issues, s.sources.prs)

// Why: the selector keeps rendering even after the user picks 'origin' (which
// collapses `sources.issues` onto origin). Upstream-candidate divergence is
// the right render gate — a repo that has an `upstream` remote pointing
// somewhere different from origin is always a candidate for the toggle,
// regardless of the current effective preference.
export const hasUpstreamCandidateDivergence = (
  s: TaskPageRepoSourceState
): s is TaskPageRepoSourceState & {
  sources: { prs: GitHubOwnerRepo; upstreamCandidate: GitHubOwnerRepo }
} =>
  !!s.sources?.prs &&
  !!s.sources.upstreamCandidate &&
  !sameGitHubOwnerRepo(s.sources.prs, s.sources.upstreamCandidate)
