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

// Why this order: the issue is linked/created BEFORE the worktree is named, so
// the name can derive from the issue title (step 3 renders tracker → name →
// agent). Step 2 only picks the repo + base branch.
export const WIZARD_STEP_LABELS = ['Host', 'Repo & branch', 'Issue & agent'] as const

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

/** Primary button label: "Create workspace" on the last step, else "Continue".
 *  While the create is in flight the last step reads "Creating…" so the button
 *  gives honest progress feedback instead of a bare spinner over an unchanged
 *  label during the (potentially slow) worktree-create + session-start (#385). */
export function wizardPrimaryLabel(step: WizardStep, creating = false): string {
  if (step === 3) {
    return creating ? 'Creating…' : 'Create workspace'
  }
  return 'Continue'
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

// ---------- Repo list (step 2) — searchable + collapsed for many projects ----------

/** How many repo rows step 2 shows before collapsing behind a "show all". Kept
 *  small so a host with many projects doesn't render a wall of rows the operator
 *  has to scroll past; the search field + expander recover the rest. */
export const REPO_LIST_COLLAPSED_CAP = 4

/** Case-insensitive filter over a repo list by display name or path. An empty
 *  query returns the list unchanged (a fresh copy). */
export function filterRepoList<T extends { displayName: string; path?: string }>(
  repos: readonly T[],
  query: string
): T[] {
  const q = query.trim().toLowerCase()
  if (!q) return [...repos]
  return repos.filter(
    (repo) =>
      repo.displayName.toLowerCase().includes(q) ||
      (repo.path ? repo.path.toLowerCase().includes(q) : false)
  )
}

/**
 * Cap the (already-filtered) repo list to `cap` rows unless expanded, so a
 * many-project host stays scannable. The currently-selected repo is always kept
 * visible even when it would fall past the cap, so a collapsed list never hides
 * the active choice. Returns the rows to render + how many stay hidden (0 when
 * expanded or already within the cap) for the "show all N" affordance.
 */
export function capRepoList<T extends { id: string }>(input: {
  repos: readonly T[]
  expanded: boolean
  selectedId: string
  cap?: number
}): { visible: T[]; hiddenCount: number } {
  const cap = input.cap ?? REPO_LIST_COLLAPSED_CAP
  if (input.expanded || input.repos.length <= cap) {
    return { visible: [...input.repos], hiddenCount: 0 }
  }
  const head = input.repos.slice(0, cap)
  // Keep the selected repo visible even if it sorts past the cap.
  if (input.selectedId && !head.some((repo) => repo.id === input.selectedId)) {
    const selected = input.repos.find((repo) => repo.id === input.selectedId)
    if (selected) {
      const visible = [...input.repos.slice(0, cap - 1), selected]
      return { visible, hiddenCount: input.repos.length - visible.length }
    }
  }
  return { visible: head, hiddenCount: input.repos.length - head.length }
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

/** Where the tracker section reads the per-repo binding from. `repoId` is the
 *  spec 020 wire identity (the server resolves the repo's own host from its
 *  registry — never a client-asserted host id); `local: false` = the repo
 *  lives on an SSH host (read-only resolution there; configuring the binding
 *  stays a local-repo affordance). */
export type TrackerBindingTarget = { workdir: string; repoId: string; local: boolean }

/**
 * Derive the binding lookup target for the selected repo. Any GIT repo gets
 * one (#356) — local repos as before, SSH repos too, carrying the repo's
 * registry id as `repoId` (the server resolves the slug on the repo's own
 * host; bindings are slug-keyed, so a binding configured on the local clone
 * serves the SSH copy too). #359 shipped this with `connectionId` as a
 * `hostId` param; migrated to spec 020's `repoId` contract at the develop
 * merge. Non-git selections resolve nothing and the picker falls back to the
 * global `activeProject`.
 */
export function deriveTrackerBindingTarget(input: {
  repo: { id: string; path: string; connectionId?: string | null } | null | undefined
  isGit: boolean
}): TrackerBindingTarget | null {
  if (!input.repo || !input.isGit) return null
  const path = input.repo.path.trim()
  if (!path) return null
  return { workdir: path, repoId: input.repo.id, local: !input.repo.connectionId }
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
  /** Locks workspace creation to the Project Hub scope that opened it. */
  requiredProjectTaskScope?: Readonly<{ scopeKey: string; generation: number; repoId: string }>
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
    initialRepoId: modalData.requiredProjectTaskScope?.repoId ?? modalData.initialRepoId,
    initialLinkedWorkItem: modalData.linkedWorkItem ?? null,
    initialWorkspaceStatus: modalData.initialWorkspaceStatus,
    initialBaseBranch: modalData.initialBaseBranch,
    telemetrySource: modalData.telemetrySource,
    ...initialStartGatedRunProp(modalData)
  }
}
