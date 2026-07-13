// Pure, React/DOM-free logic behind the wizard's "create issue from intent"
// sub-panel (spec 013 F2/F3). The heavy lifting — drafting, filing, binding —
// stays in `useComposerState`'s create-issue seams (`onGenerateIssueBody` →
// `draftGithubIssueBody`, `onCreateIssueSubmit` → `createGithubIssue` →
// `applyLinkedWorkItem`); this module only maps the hook's flags to a display
// phase and gates the two buttons, so the sub-panel is unit-testable without a
// DOM (the UI package ships no jsdom).
import { deriveGoalIssueDraft } from '@/lib/workspace-goal-step'
import { deriveIssueSideEffectGate } from '@/lib/issue-side-effect-gate'
import type { IssueSideEffectGate } from '@/lib/issue-side-effect-gate'
import type { PickerProjectRef } from './work-item-picker-model'

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

/** Which provider "Create issue" files into. */
export type CreateIssueProvider = 'github' | 'linear'

/**
 * Spec 013 F3: which tracker "Create issue" targets. Follows the resolved
 * tracker's provider (open question 2, decisive default):
 *  - a resolved GitHub Project ⇒ `github`;
 *  - no Project but Linear connected ⇒ `linear`;
 *  - BOTH a Project AND Linear ⇒ `ambiguous` (the sub-panel shows a provider
 *    toggle so the operator disambiguates);
 *  - neither ⇒ `github` — the default create path; the GitHub arm surfaces the
 *    honest no-repo / no-credential error inline (never silently misfiles).
 * The drafted body is provider-agnostic; only the *create* call branches.
 */
export function resolveCreateIssueProvider(input: {
  resolved: PickerProjectRef | null
  linearConnected: boolean
}): CreateIssueProvider | 'ambiguous' {
  if (input.resolved && input.linearConnected) return 'ambiguous'
  if (input.resolved) return 'github'
  if (input.linearConnected) return 'linear'
  return 'github'
}

// ---------- Spec 015 F3: Tracker-tab intake (add-only extensions) ----------
// The Project Hub's Tracker panel reuses the 013 phase/gating helpers above
// unchanged; the additions below cover what the wizard panel never needed — a
// terminal `filed` phase (the panel outlives the file, offering "Start gated
// run") and gated-run eligibility for the *filed* issue.

/** The Tracker panel's phase — 013's `CreateIssueIntentPhase` plus `filed`. */
export type TrackerIntakePhase = 'idle' | 'drafting' | 'review' | 'filing' | 'filed' | 'error'

/**
 * A provider-CONFIRMED created issue. Set only from a create response carrying
 * an id/URL — never optimistically — so the panel can't show a phantom
 * "filed" (spec 015 AC 12).
 */
export type FiledIssue =
  | { provider: 'github'; number: number; url: string; slug: string; title: string }
  | { provider: 'linear'; identifier: string; url: string | null; title: string }

/**
 * Precedence: filing > drafting > error > filed > review(hasBody) > idle.
 * `filed` must beat `review` (the drafted body is still in hand after a
 * successful file); a new Draft resets `filed` (hook contract), so a stale
 * "filed" chip can never sit over a fresh draft.
 */
export function deriveTrackerIntakePhase(s: {
  generating: boolean
  submitting: boolean
  error: string | null
  hasBody: boolean
  filed: FiledIssue | null
}): TrackerIntakePhase {
  if (s.submitting) return 'filing'
  if (s.generating) return 'drafting'
  if (s.error) return 'error'
  if (s.filed) return 'filed'
  if (s.hasBody) return 'review'
  return 'idle'
}

/**
 * Gated-run eligibility for a filed issue: composes the SAME
 * `deriveIssueSideEffectGate` the wizard submits through, so the panel's
 * "Start gated run" renders exactly when the wizard's toggle would arm. A
 * Linear identifier/URL fails the github.com parse and returns
 * `not-github-url` — honest by construction (D3: gated runs are GitHub-only).
 */
export function deriveFiledGatedRunGate(
  filed: FiledIssue | null,
  repoConnectionId: string | null | undefined
): IssueSideEffectGate {
  return deriveIssueSideEffectGate(
    filed ? { type: 'issue', url: filed.url ?? '' } : null,
    repoConnectionId
  )
}
