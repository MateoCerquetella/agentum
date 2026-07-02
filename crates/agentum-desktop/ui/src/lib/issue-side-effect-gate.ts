import { parseGitHubIssueOrPRLink } from '@/lib/github-links'
import type { RepoSlug } from '@/lib/github-links'

// Spec 007 (bug 2): the composer's issue side effects — "Scaffold spec"
// (spec 004 F4) and "Start gated run" (spec 005 F1) — were gated in four
// places (toggle render + two submit paths + the two post-create callbacks)
// with slightly diverging copies, and every failing gate returned SILENTLY.
// This is the single, pure derivation all of them share, and the reason
// string is what the armed-but-skipped toast names.

export type IssueSideEffectSkipReason =
  | 'no-linked-item'
  | 'not-an-issue'
  | 'not-github-url'
  | 'remote-repo'

export type IssueSideEffectGate =
  | { eligible: true; slug: RepoSlug; number: number }
  | { eligible: false; reason: IssueSideEffectSkipReason }

/**
 * Eligibility for the linked-issue side effects: a linked *github.com issue*
 * targeting a *local* repo (the scaffold/start-work endpoints write into the
 * new worktree's local path).
 */
export function deriveIssueSideEffectGate(
  item: { type: string; url: string } | null | undefined,
  repoConnectionId: string | null | undefined
): IssueSideEffectGate {
  if (!item) {
    return { eligible: false, reason: 'no-linked-item' }
  }
  if (item.type !== 'issue') {
    return { eligible: false, reason: 'not-an-issue' }
  }
  const link = parseGitHubIssueOrPRLink(item.url)
  if (!link || link.type !== 'issue') {
    return { eligible: false, reason: 'not-github-url' }
  }
  if (repoConnectionId) {
    return { eligible: false, reason: 'remote-repo' }
  }
  return { eligible: true, slug: link.slug, number: link.number }
}

const SKIP_REASON_COPY: Record<IssueSideEffectSkipReason, string> = {
  'no-linked-item': 'no issue is linked to this workspace anymore.',
  'not-an-issue': 'the linked item is not an issue.',
  'not-github-url': "the linked issue's URL is not a github.com issue link.",
  'remote-repo': 'the selected repo is remote (SSH) — this runs locally only.'
}

/**
 * The toast copy for an ARMED toggle whose submit-time gate failed. Silent
 * skips are exactly the bug this replaces — always surface the reason.
 */
export function describeIssueSideEffectSkip(
  action: 'scaffold-spec' | 'start-gated-run',
  reason: IssueSideEffectSkipReason
): string {
  const label = action === 'scaffold-spec' ? 'Spec scaffold skipped' : 'Gated run not started'
  return `${label}: ${SKIP_REASON_COPY[reason]}`
}
