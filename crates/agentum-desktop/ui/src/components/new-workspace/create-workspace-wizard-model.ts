// Pure, React/DOM-free logic behind `CreateWorkspaceWizard`. The UI package
// ships no jsdom, so the wizard's gradeable behaviors — step gating, the header
// recap, the agent-pill fallback, and the footer copy — live here where they
// can be unit-tested without mounting the component (mirrors the goal step's
// `workspace-goal-step.ts`). The component owns only local state + JSX.
import { filterEnabledTuiAgents, isTuiAgentEnabled } from '../../../../shared/tui-agent-selection'
import type {
  TuiAgent,
  WorkspaceCreateTelemetrySource,
  WorkspaceStatus
} from '../../../../shared/types'
import type { LinkedWorkItemSummary } from '@/lib/new-workspace'
import { initialStartGatedRunProp } from '@/lib/composer-modal-props'
import type { PickerProjectRef } from './work-item-picker-model'

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

/** The label shown on the base-branch combobox trigger: the chosen ref, else
 *  the repo's resolved default ref, else a generic "default branch" hint. */
export function wizardBaseBranchTriggerLabel(
  baseBranch: string | undefined,
  defaultRef: string | null | undefined
): string {
  const chosen = baseBranch?.trim()
  if (chosen) return chosen
  const fallback = defaultRef?.trim()
  return fallback || 'default branch'
}

// ---------- Unified tracker (spec 013 F1 — one honest source) ----------

/**
 * Step-3's merged tracker section is driven SOLELY by the Project the issue
 * picker resolves (`resolvePickerProject` = per-repo binding ∨ global
 * `activeProject`). There is deliberately no second detection path (a git-remote
 * heuristic used to drive a separate "Tracker" card and could disagree with the
 * picker — the exact contradiction AC 3 forbids). The five states below are all
 * a function of that one `resolved` value plus the picker's fetch status:
 * - `connected`: a Project resolved and its open issues loaded.
 * - `connecting`: resolved, the issue fetch is in flight.
 * - `connected-empty`: resolved and loaded, but the Project has 0 open issues.
 * - `unavailable`: resolved (still connected) but the issue fetch failed.
 * - `none`: no Project resolved → the honest "no tracker (optional)" state.
 */
export type UnifiedTrackerStatus =
  | { kind: 'connected'; issueCount: number }
  | { kind: 'connecting' }
  | { kind: 'connected-empty' }
  | { kind: 'unavailable' }
  | { kind: 'none' }

/**
 * Derive the merged tracker section's status from the picker's own resolution.
 * `resolved == null` is the ONLY input that yields `none`; a null resolution
 * forces `deriveIssueOptions([])` (zero issues) upstream, so "none" can never
 * coexist with a non-empty issue list (AC 3 — structural, not a runtime check).
 */
export function deriveUnifiedTrackerStatus(input: {
  resolved: PickerProjectRef | null
  status: 'idle' | 'loading' | 'failed'
  optionCount: number
}): UnifiedTrackerStatus {
  if (!input.resolved) return { kind: 'none' }
  if (input.status === 'loading') return { kind: 'connecting' }
  if (input.status === 'failed') return { kind: 'unavailable' }
  if (input.optionCount <= 0) return { kind: 'connected-empty' }
  return { kind: 'connected', issueCount: input.optionCount }
}

// ---------- Single front door (spec 013 F4) ----------

/**
 * The modal-open data the wizard honors. Widened (spec 013 F4) from the plain
 * subset to the full `ComposerModalData` shape so the wizard is the SINGLE
 * front door: every opinionated open (`startGatedRun`, `linkedWorkItem`,
 * `initialBaseBranch`, `initialWorkspaceStatus`, …) reaches an equivalent
 * create through the wizard, with no lost capability (the composer card + goal
 * step are removed).
 */
export type CreateWorkspaceWizardData = {
  prefilledName?: string
  initialRepoId?: string
  linkedWorkItem?: LinkedWorkItemSummary | null
  initialBaseBranch?: string
  initialWorkspaceStatus?: WorkspaceStatus
  /** Spec 005 F1 (AC 3): open with the "Start gated run" toggle armed. */
  startGatedRun?: boolean
  telemetrySource?: WorkspaceCreateTelemetrySource
}

/**
 * Map the modal-open data onto the `useComposerState` seed the wizard passes
 * (spec 013 F4). Pure so each opinionated field's honoring is unit-pinned — a
 * caller's `linkedWorkItem` / `initialBaseBranch` / `initialWorkspaceStatus` /
 * `startGatedRun` can never silently fail to seed. `initialStartGatedRun` rides
 * through the existing `initialStartGatedRunProp` seam (inv. 4), so an armed
 * open opens the toggle already armed and submits via the same gated path.
 */
export function deriveWizardComposerSeed(modalData: CreateWorkspaceWizardData): {
  initialName: string
  initialRepoId: string | undefined
  initialLinkedWorkItem: LinkedWorkItemSummary | null
  initialWorkspaceStatus: WorkspaceStatus | undefined
  initialBaseBranch: string | undefined
  telemetrySource: WorkspaceCreateTelemetrySource | undefined
} & ({ initialStartGatedRun: true } | Record<string, never>) {
  return {
    initialName: modalData.prefilledName ?? '',
    initialRepoId: modalData.initialRepoId,
    initialLinkedWorkItem: modalData.linkedWorkItem ?? null,
    initialWorkspaceStatus: modalData.initialWorkspaceStatus,
    initialBaseBranch: modalData.initialBaseBranch,
    telemetrySource: modalData.telemetrySource,
    ...initialStartGatedRunProp(modalData)
  }
}
