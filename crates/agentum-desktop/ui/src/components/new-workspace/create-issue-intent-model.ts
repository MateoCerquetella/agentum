// Pure, React/DOM-free logic behind the wizard's "create issue from intent"
// sub-panel (spec 013 F2/F3). The heavy lifting — drafting, filing, binding —
// stays in `useComposerState`'s create-issue seams (`onGenerateIssueBody` →
// `draftGithubIssueBody`, `onCreateIssueSubmit` → `createGithubIssue` →
// `applyLinkedWorkItem`); this module only maps the hook's flags to a display
// phase and gates the two buttons, so the sub-panel is unit-testable without a
// DOM (the UI package ships no jsdom).
import { deriveGoalIssueDraft } from '@/lib/workspace-goal-step'

/**
 * The sub-panel's high-level phase, a pure function of the hook's create-issue
 * flags:
 * - `filing`: the create call is in flight (`submitting`).
 * - `drafting`: the body draft is in flight (`generating`).
 * - `error`: the last draft/file failed (an inline message is shown; the form
 *   stays usable for a retry — never blocks the wizard's Create workspace).
 * - `review`: a body has been drafted and awaits review/edit before filing.
 * - `idle`: nothing drafted yet — awaiting an intent + Draft.
 *
 * Busy states win over `error` (an error is only set once the op settles), and
 * `error` wins over `review`/`idle` so the banner is not hidden by stale body.
 */
export type CreateIssueIntentPhase = 'idle' | 'drafting' | 'review' | 'filing' | 'error'

export function deriveCreateIssueIntentPhase(s: {
  generating: boolean
  submitting: boolean
  error: string | null
  hasBody: boolean
}): CreateIssueIntentPhase {
  if (s.submitting) return 'filing'
  if (s.generating) return 'drafting'
  if (s.error) return 'error'
  if (s.hasBody) return 'review'
  return 'idle'
}

/** Can we draft a body from the typed intent? Non-blank intent, not busy. */
export function canDraftIssue(intent: string, busy: boolean): boolean {
  return intent.trim().length > 0 && !busy
}

/** Can we file the issue? A title is present and nothing is in flight. */
export function canFileIssue(title: string, busy: boolean): boolean {
  return title.trim().length > 0 && !busy
}

/** Intent → seed title (reuses `deriveGoalIssueDraft` so the description alone
 *  produces a titled draft, exactly like the goal step's issue seed). */
export function deriveIntentTitle(intent: string): string {
  return deriveGoalIssueDraft(intent).title
}
