// Spec 008 F3 (AC 9–11): goal-first "Create New Workspace". This module is the
// PURE, React/DOM/xterm-free logic behind `NewWorkspaceGoalStep` so the three
// gradeable behaviors are unit-testable without a DOM (the UI package ships no
// jsdom — interactive logic is deliberately extracted here):
//   - the goal → composer-seed mapping (AC 9, "seed name/prompt from the goal"),
//   - which inputs are required vs optional (AC 10, "goal + a workdir target are
//     the only required inputs"; worktree-creation / scaffold / tracker skippable),
//   - the default-first-screen + "Skip to details" reveal decision (AC 9/D3).
//
// D-C / D3: the goal step FRONTS the existing composer (`useComposerState`) and
// hands it values via props — it never becomes a wizard state-machine inside the
// creation engine. This module owns only the pure transforms; the component owns
// the local state and the composer stays the untouched creation engine.

/**
 * Derive a concise, worktree-safe workspace name from free-text goal input.
 * Lowercased, punctuation collapsed to word boundaries, first `maxWords` joined
 * with `-`, clamped so the seeded name field stays short. Empty for a
 * blank/whitespace goal so the composer falls back to its own default name.
 */
export function slugifyGoalName(goal: string, maxWords = 6): string {
  const words = goal
    .toLowerCase()
    // Keep only slug-safe characters; everything else becomes a boundary.
    .replace(/[^a-z0-9\s-]+/g, ' ')
    .split(/[\s-]+/)
    .filter(Boolean)
    .slice(0, Math.max(0, maxWords))
  return words
    .join('-')
    .slice(0, 48)
    // A trailing '-' can survive the length clamp mid-word; drop it.
    .replace(/-+$/, '')
}

/**
 * The composer seed produced from a captured goal (AC 9). `name` seeds the
 * workspace-name field; `prompt` is the verbatim goal (the agent prompt / the
 * source for the tracker issue body). `goal` is kept as the trimmed source of
 * truth so downstream seeds (the issue draft) derive from one value.
 */
export type WorkspaceGoalSeed = {
  goal: string
  name: string
  prompt: string
}

/** Map goal text → the composer seed (pure; the AC 9 "seed" behavior). */
export function deriveWorkspaceGoalSeed(goal: string): WorkspaceGoalSeed {
  const trimmed = goal.trim()
  return { goal: trimmed, name: slugifyGoalName(trimmed), prompt: trimmed }
}

/**
 * A GitHub-issue draft seeded from the goal. The tracker step (c) reuses the
 * composer's existing create-issue form pre-filled with this so a workspace can
 * reach `start_work`'s precondition set (issue → scaffold → gated run) with no
 * retyping (AC 11 "without further setup"). Title = the goal's first line
 * (truncated); body = the whole goal.
 */
export type GoalIssueDraft = { title: string; body: string }

const ISSUE_TITLE_MAX = 72

export function deriveGoalIssueDraft(goal: string): GoalIssueDraft {
  const trimmed = goal.trim()
  const firstLine = (trimmed.split(/\r?\n/, 1)[0] ?? '').trim()
  const title =
    firstLine.length > ISSUE_TITLE_MAX ? `${firstLine.slice(0, ISSUE_TITLE_MAX - 1).trimEnd()}…` : firstLine
  return { title, body: trimmed }
}

/**
 * The goal step's inputs. D9: a session is `(name, workdir, …)`, so goal + a
 * workdir target (`repoId`) are the two REQUIRED inputs — worktree *creation* is
 * an optional step, not the workdir.
 */
export type GoalStepInputs = { goal: string; repoId: string }

/** True iff both required inputs are present (AC 10 required set). */
export function isGoalStepReady({ goal, repoId }: GoalStepInputs): boolean {
  return goal.trim().length > 0 && repoId.trim().length > 0
}

/**
 * The first unmet required input as a user-facing message, or null when ready.
 * Goal is checked before workdir (goal-first ordering, AC 9) — and it is never
 * silent: an unmet requirement always names itself.
 */
export function firstGoalStepBlocker({ goal, repoId }: GoalStepInputs): string | null {
  if (goal.trim().length === 0) return 'Describe your goal to continue.'
  if (repoId.trim().length === 0) return 'Pick a project (workdir) to continue.'
  return null
}

/**
 * The four SKIPPABLE steps offered after the goal (AC 10; spec 010 F3 appended
 * `provision`). Held as data so the "each is optional / none blocks creation"
 * invariant is unit-pinned, and each names the EXISTING composer/server
 * primitive it reuses (reuse, don't rebuild): the fresh-worktree creation
 * (skip → an existing folder/branch as-is), the spec scaffold, the tracker
 * binding, and the repo provisioning ensure (labels + board + harness
 * scaffold — the modal's `'provision'` phase).
 */
type OptionalWorkspaceStepId = 'worktree' | 'scaffold' | 'tracker' | 'provision'

export type OptionalWorkspaceStep = {
  id: OptionalWorkspaceStepId
  label: string
  /** Every one of the three is skippable — none blocks creation (AC 10). */
  skippable: true
  /** The existing composer/server primitive this step reuses. */
  primitive: string
}

export const OPTIONAL_WORKSPACE_STEPS: readonly OptionalWorkspaceStep[] = [
  {
    id: 'worktree',
    label: 'Create a fresh worktree',
    skippable: true,
    primitive: 'createWorktree'
  },
  {
    id: 'scaffold',
    label: 'Scaffold a spec from the issue',
    skippable: true,
    primitive: 'maybeScaffoldSpecFromIssue'
  },
  {
    id: 'tracker',
    label: 'File or link a tracker issue',
    skippable: true,
    primitive: 'createGithubIssue'
  },
  {
    id: 'provision',
    label: 'Provision the repo (labels, board, scaffold)',
    skippable: true,
    primitive: 'provisionWorkspace'
  }
] as const

/** The modal's two screens; the goal step is the DEFAULT first screen (AC 9). */
export type ComposerModalPhase = 'goal' | 'details'
export const DEFAULT_COMPOSER_MODAL_PHASE: ComposerModalPhase = 'goal'

/**
 * Whether a modal open should START at the goal step. The goal-first framing is
 * the DEFAULT plain-create entry (Cmd+J / sidebar "+" / shortcut). An
 * "opinionated" open — the Tasks-page pre-armed gated-run hop (`startGatedRun`),
 * a create-from linked item, a prefilled name, or a pinned base branch — already
 * has its intent and skips straight to the mechanics-first details screen (D3:
 * the composer stays reachable). This keeps F1's Tasks hop byte-identical.
 */
export function shouldStartAtGoalStep(modalData: {
  startGatedRun?: boolean
  linkedWorkItem?: unknown
  prefilledName?: string
  initialBaseBranch?: string
}): boolean {
  if (modalData.startGatedRun) return false
  if (modalData.linkedWorkItem) return false
  if (modalData.prefilledName && modalData.prefilledName.trim().length > 0) return false
  if (modalData.initialBaseBranch && modalData.initialBaseBranch.trim().length > 0) return false
  return true
}

/** The initial screen for a given modal open (AC 9 default; D3 reachability). */
export function initialComposerPhase(modalData: {
  startGatedRun?: boolean
  linkedWorkItem?: unknown
  prefilledName?: string
  initialBaseBranch?: string
}): ComposerModalPhase {
  return shouldStartAtGoalStep(modalData) ? 'goal' : 'details'
}

/** The user action that leaves the goal step. */
export type GoalStepAction = { kind: 'continue'; goal: string } | { kind: 'skip' }

/** The revealed details screen + the seed to apply (null = no goal framing). */
export type ComposerReveal = { phase: 'details'; seed: WorkspaceGoalSeed | null }

/**
 * The reveal decision (AC 9), pure so it is unit-testable state: "Continue"
 * reveals the details screen seeded from the goal; "Skip to details" (D3)
 * reveals it with no seed — byte-identical to today's mechanics-first behavior.
 */
export function revealDetails(action: GoalStepAction): ComposerReveal {
  if (action.kind === 'skip') return { phase: 'details', seed: null }
  return { phase: 'details', seed: deriveWorkspaceGoalSeed(action.goal) }
}
