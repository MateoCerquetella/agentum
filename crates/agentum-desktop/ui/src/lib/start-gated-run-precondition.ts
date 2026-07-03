// Spec 008 F1 #2 (the #226 chat-origin `repoId: ''` edge): the composer's
// `submit` guard early-returns when a precondition isn't met. When "Start gated
// run" is ARMED, a bare `return` is a silent no-op — exactly the AC-1 "never
// silent" failure. This pure helper names the FIRST unmet precondition (in the
// same order the guard checks them) so `submit` can toast it instead. Pure so
// the "armed guard is never silent" behavior is unit-tested without the
// 2.8k-line hook (no toast/DOM/xterm imports — keeps vitest fast and quiet).

export interface StartGatedRunPreconditionState {
  /** The selected repo id. The #226 edge is `repoId: ''` (falsy). */
  repoId: string | null | undefined
  /** The derived workspace name seed. */
  workspaceSeedName: string | null | undefined
  /** Whether a repo object is currently selected. */
  hasSelectedRepo: boolean
  /** The selected repo is remote and its connection isn't ready. */
  selectedRepoRequiresConnection: boolean
  /** Still probing repo setup — submit is deferred, not blocked. */
  shouldWaitForSetupCheck: boolean
  /** Still probing issue automation — submit is deferred, not blocked. */
  shouldWaitForIssueAutomationCheck: boolean
  /** The repo needs an explicit setup choice… */
  requiresExplicitSetupChoice: boolean
  /** …and one hasn't been made yet. */
  hasSetupDecision: boolean
  /** A sparse-checkout validation error, if any. */
  sparseError: string | null
}

/**
 * The first unmet precondition that blocks a gated-run submit, as a user-facing
 * message — or `null` when every precondition is met. The check order mirrors
 * the composer `submit` guard's `||` chain so the reported blocker is the one
 * that actually tripped.
 */
export function firstStartGatedRunBlocker(s: StartGatedRunPreconditionState): string | null {
  if (!s.repoId || !s.hasSelectedRepo) {
    return 'Pick a repo before starting a gated run.'
  }
  if (!s.workspaceSeedName) {
    return 'Name the workspace before starting a gated run.'
  }
  if (s.selectedRepoRequiresConnection) {
    return 'Connect to the repo’s host before starting a gated run.'
  }
  if (s.shouldWaitForSetupCheck || s.shouldWaitForIssueAutomationCheck) {
    return 'Still checking project setup — try again in a moment.'
  }
  if (s.requiresExplicitSetupChoice && !s.hasSetupDecision) {
    return 'Choose a setup option before starting a gated run.'
  }
  if (s.sparseError !== null) {
    return s.sparseError
  }
  return null
}
