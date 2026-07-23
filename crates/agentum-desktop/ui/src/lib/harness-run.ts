// Spec 023 — pure, IO-free helpers over a live `HarnessStatus`: match an
// engine run to a worktree by workdir (Part A, architecture Q1's
// by-`harness_id` surface) and read the run's linked tracker issue (Part B).
// No store/runtime imports — the IO shell lives in
// `hooks/useWorktreeHarnessRun.ts`, the surfaces in `components/gated-run/`.
import type { HarnessStatus } from '@/runtime/harness-client'
import { normalizeWorkdir } from './workspace-harness-detect'

/**
 * The engine run whose `workdir` is the worktree's path. Both sides are
 * normalized exactly like the spec-015 offer dedupe (`decideHarnessOffer`):
 * `HarnessStatus.workdir` is the server's `expand_workdir`'d absolute path,
 * `worktree.path` comes back pre-expanded — a trailing-slash spelling still
 * matches, symlink divergence stays an accepted residual (same exposure as
 * the engine's own `find_by_workdir`).
 */
export function findHarnessRunForWorkdir(
  runs: HarnessStatus[],
  workdir: string
): HarnessStatus | undefined {
  const normalized = normalizeWorkdir(workdir)
  return runs.find((r) => normalizeWorkdir(r.workdir) === normalized)
}

/**
 * Part B (AC 7): the run's linked tracker issue URL — a mirror of the
 * server's `shared_tracker_provenance` (`harness/types.rs:247`): the stamp
 * loop writes provider+url uniformly across the backlog, so the first feature
 * carrying BOTH speaks for the run. `null` once unlinked (or never stamped).
 */
export function runLinkedIssue(status: HarnessStatus): string | null {
  for (const f of status.features.features) {
    if (f.tracker_provider && f.tracker_url) {
      return f.tracker_url
    }
  }
  return null
}

/** Short chip label for a linked-issue URL: `#42` for a GitHub issue URL,
 *  the raw URL otherwise (Linear identifiers already read as labels). */
export function linkedIssueLabel(url: string): string {
  const m = /\/issues\/(\d+)(?:[/?#]|$)/.exec(url)
  return m ? `#${m[1]}` : url
}

export type GatedRunStageStatus = 'complete' | 'active' | 'blocked' | 'paused' | 'upcoming'

export type GatedRunStage = {
  id: 'authoring' | 'architecture' | 'decompose' | 'executing' | 'review'
  label: string
  status: GatedRunStageStatus
}

const STAGES: Array<Pick<GatedRunStage, 'id' | 'label'>> = [
  { id: 'authoring', label: 'PM spec' },
  { id: 'architecture', label: 'Architecture' },
  { id: 'decompose', label: 'Plan tasks' },
  { id: 'executing', label: 'Build' },
  { id: 'review', label: 'Review' }
]

export function gatedRunPhaseLabel(phase: HarnessStatus['phase'] | null | undefined): string {
  switch (phase) {
    case 'authoring':
      return 'PM spec gate'
    case 'architecture':
      return 'architecture gate'
    case 'decompose':
      return 'task planning'
    case 'executing':
      return 'build'
    case 'review':
      return 'review gate'
    case 'awaiting_confirm':
      return 'human confirmation'
    case 'done':
      return 'complete'
    case 'blocked':
      return 'blocked gate'
    default:
      return 'gated run'
  }
}

/** The SDD role rail. Plain harness runs omit it rather than pretending they
 * completed PM/architecture phases that were disabled. */
export function deriveGatedRunStages(status: HarnessStatus): GatedRunStage[] {
  if (!status.features.roles) return []
  if (status.phase === 'done') {
    return STAGES.map((stage) => ({ ...stage, status: 'complete' }))
  }
  const effective =
    status.phase === 'blocked' || status.phase === 'awaiting_confirm'
      ? status.blocked_phase
      : status.phase
  const activeIndex = STAGES.findIndex((stage) => stage.id === effective)
  return STAGES.map((stage, index) => ({
    ...stage,
    status:
      activeIndex < 0
        ? 'upcoming'
        : index < activeIndex
          ? 'complete'
          : index > activeIndex
            ? 'upcoming'
            : status.phase === 'blocked'
              ? 'blocked'
              : status.phase === 'awaiting_confirm'
                ? 'paused'
                : 'active'
  }))
}

export function currentHarnessFeature(status: HarnessStatus) {
  return status.features.features.find((feature) => feature.id === status.current_feature) ?? null
}

/** Human status copy for the workspace strip. Raw enum concatenation caused
 * the `blocked · blocked` failure this surface replaces. */
export function gatedRunHeadline(status: HarnessStatus): string {
  const feature = currentHarnessFeature(status)
  if (status.state === 'blocked') {
    if (feature?.state === 'blocked') return `Blocked on ${feature.name}`
    return `${gatedRunPhaseLabel(status.blocked_phase)} needs attention`
  }
  if (status.state === 'awaiting_confirmation') {
    return `${gatedRunPhaseLabel(status.blocked_phase ?? status.phase)} is waiting for you`
  }
  if (status.state === 'failed') return 'Gated run failed'
  if (status.state === 'done') return 'Gated run complete'
  if (status.state === 'init_verifying') return 'Checking the workspace environment'
  if (feature?.state === 'verifying') return `Verifying ${feature.name}`
  if (feature?.state === 'ready_to_test') return `Browser QA for ${feature.name}`
  if (status.state === 'verifying') return 'Running the verification gate'
  switch (status.phase) {
    case 'authoring':
      return 'PM is shaping the spec'
    case 'architecture':
      return 'Architect is designing the approach'
    case 'decompose':
      return 'Turning the spec into tasks'
    case 'review':
      return 'Reviewer is checking the completed work'
    case 'executing':
      return feature ? `Working on ${feature.name}` : 'Preparing the next task'
    default:
      return status.state === 'idle' ? 'Gated run queued' : 'Gated run active'
  }
}

export function gatedRunBlocker(status: HarnessStatus): string | null {
  const feature = currentHarnessFeature(status)
  if (feature?.state === 'blocked' && feature.last_error) return feature.last_error
  return status.gate_summary?.trim() || null
}

/** Short title for each engine session tab. */
export function gatedRunSessionTitle(status: HarnessStatus): string {
  const feature = currentHarnessFeature(status)
  if (feature?.state === 'ready_to_test') return `QA · ${feature.id}`
  if (feature) return `${feature.id} · Gated run`
  switch (status.phase) {
    case 'authoring':
      return 'PM · Gated run'
    case 'architecture':
      return 'Architect · Gated run'
    case 'review':
      return 'Review · Gated run'
    default:
      return 'Gated run'
  }
}

/** What the workspace view shows while an owned gated run boots (AC 1). */
export type GatedRunSurface = 'starting' | 'session' | 'picker'

/**
 * Part A (AC 1–3): the surfacing decision for a surface-less workspace.
 *
 * - `hasAttachableSession` → `'session'`: a live session always wins; the
 *   normal session view takes over and the pending slice clears.
 * - no pending slice → `'picker'`: today's behavior, including the
 *   non-ownership fallback path (AC 3 — `gatedRunResultOwnsWorktree` false
 *   never writes the slice).
 * - pending + engine session present → `'session'`: the run's agent exists;
 *   its tab flips the workspace off the empty state.
 * - pending + halted run (`done`/`failed`/`blocked`) → `'picker'`: nothing is
 *   starting anymore; the loud failure toast already fired via
 *   `subscribeHarnessRunErrors` (AC 3).
 * - otherwise (pending, run missing-or-booting) → `'starting'`: render the
 *   "Gated run starting…" state reflecting the live `HarnessState`/feature.
 */
export function deriveGatedRunSurface(input: {
  pendingGatedRun: boolean
  harness: HarnessStatus | undefined
  hasAttachableSession: boolean
}): GatedRunSurface {
  if (input.hasAttachableSession) {
    return 'session'
  }
  if (!input.pendingGatedRun) {
    return 'picker'
  }
  const harness = input.harness
  if (harness) {
    if (harness.current_session) {
      return 'session'
    }
    if (harness.state === 'done' || harness.state === 'failed' || harness.state === 'blocked') {
      return 'picker'
    }
  }
  return 'starting'
}
