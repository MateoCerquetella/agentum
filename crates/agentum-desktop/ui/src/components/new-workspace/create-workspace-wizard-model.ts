// Pure, React/DOM-free logic behind `CreateWorkspaceWizard`. The UI package
// ships no jsdom, so the wizard's gradeable behaviors — step gating, the header
// recap, the agent-pill fallback, and the footer copy — live here where they
// can be unit-tested without mounting the component (mirrors the goal step's
// `workspace-goal-step.ts`). The component owns only local state + JSX.
import { filterEnabledTuiAgents, isTuiAgentEnabled } from '../../../../shared/tui-agent-selection'
import type { TuiAgent } from '../../../../shared/types'

export type WizardStep = 1 | 2 | 3

export const WIZARD_STEP_LABELS = ['Host', 'Repo & worktree', 'Agent & tracker'] as const

// Fallback agent pills for when detection hasn't produced a set yet (or found
// nothing installed) — kept small and catalog-ordered so the picker is never
// empty while `ensureDetectedAgents` is still in flight.
export const WIZARD_FALLBACK_AGENT_IDS: TuiAgent[] = ['claude', 'codex', 'gemini']

/** Step 2 (repo) can advance only once a repo is chosen and it doesn't still
 *  need an SSH connection — otherwise the create would fail at the gate. */
export function canLeaveRepoStep(input: {
  repoId: string
  requiresConnection: boolean
}): boolean {
  return Boolean(input.repoId) && !input.requiresConnection
}

/**
 * The agent pills to show. Prefer the enabled subset of the detected set; fall
 * back to the enabled catalog defaults so the picker is never empty (detection
 * is best-effort and can still be in flight or empty on a fresh host).
 */
export function resolveWizardAgentOptions(input: {
  detectedAgentIds: Iterable<TuiAgent> | null
  disabledTuiAgents?: Iterable<unknown> | null
  fallback?: TuiAgent[]
}): TuiAgent[] {
  const fallback = input.fallback ?? WIZARD_FALLBACK_AGENT_IDS
  if (input.detectedAgentIds) {
    const detected = [...input.detectedAgentIds].filter((id) =>
      isTuiAgentEnabled(id, input.disabledTuiAgents)
    )
    if (detected.length > 0) {
      return detected
    }
  }
  return filterEnabledTuiAgents(fallback, input.disabledTuiAgents)
}

/**
 * The truncatable header recap: the host, then repo·worktree once past step 1,
 * then the agent once past step 2 — mirrors what's been decided so far. Skips
 * absent pieces (blank worktree name, no agent) so it never shows dangling
 * separators.
 */
export function buildWizardRecap(input: {
  step: WizardStep
  hostLabel: string
  repoDisplayName?: string | null
  worktreeName?: string | null
  agent?: TuiAgent | null
}): string {
  const parts: string[] = [input.hostLabel]
  if (input.step > 1 && input.repoDisplayName) {
    const name = input.worktreeName?.trim()
    parts.push(name ? `${input.repoDisplayName} · ${name}` : input.repoDisplayName)
  }
  if (input.step > 2 && input.agent) {
    parts.push(input.agent)
  }
  return parts.join('  ·  ')
}

/** Primary button label: "Create workspace" on the last step, else "Continue". */
export function wizardPrimaryLabel(step: WizardStep): string {
  return step === 3 ? 'Create workspace' : 'Continue'
}

/** The muted "what's next" hint shown beside the primary button. */
export function wizardNextHint(step: WizardStep, hostLabel: string): string {
  if (step === 1) return `Next: repos on ${hostLabel}`
  if (step === 2) return 'Next: agent & tracker'
  return 'Lands you in a fresh session'
}
