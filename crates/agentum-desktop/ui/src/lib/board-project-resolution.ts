// Spec 016: which GitHub Project a board surface shows, per repo. Precedence
// is D1's, exactly: explicit per-repo pick → server tracker binding → legacy
// global `activeProject` (the ONLY new-code read of that slot) → none. Pure —
// no React/store/DOM imports — so verify.sh asserts the precedence, the
// pending/divergence semantics, and the settings-write sibling preservation
// (`applyBoardPick`/`clearBoardPick`) instead of trusting review. It
// generalizes `resolvePickerProject` (spec 011 F2, work-item-picker-model.ts):
// same binding-identity normalization (complete identity only; ownerType
// exact-matches 'organization' else 'user') plus the pick tier on top — the
// wizard's resolver stays untouched because its binding-first order is what
// status automation writes against.
import type { GitHubProjectOwnerType, GitHubProjectSettings } from '@/shared/github-project-types'

export type BoardProjectRef = {
  owner: string
  ownerType: GitHubProjectOwnerType
  number: number
}

/** The identity fields read off ProjectBindingDto (free-string ownerType on the wire). */
export type BoardBindingIdentity = {
  projectOwner: string | null
  projectOwnerType: string | null
  projectNumber: number | null
  projectTitle?: string | null
}

/** The session cache entry the hub effect writes and consumers read. A fetch
 *  FAILURE is written as `{status:'loaded', binding:null}` (fail-closed to the
 *  legacy tier), so `pending` can never wedge. */
export type BoardBindingState =
  | { status: 'loading' }
  | { status: 'loaded'; binding: BoardBindingIdentity | null }

export type BoardProjectResolution =
  /** Explicit per-repo pick wins (D1). `divergesFromBinding` is the BOUND
   *  project when the pick differs from a complete, loaded binding — drives
   *  the non-blocking hint; null otherwise (including while binding loads). */
  | { source: 'pick'; project: BoardProjectRef; divergesFromBinding: BoardProjectRef | null }
  | { source: 'binding'; project: BoardProjectRef }
  | { source: 'legacy'; project: BoardProjectRef }
  /** Nothing resolved → plain issue Kanban; NEVER force githubMode 'project'. */
  | { source: 'none'; project: null }
  /** No pick and the binding fetch is in flight → hold (skeleton), do NOT
   *  flash the legacy project then swap. */
  | { source: 'pending'; project: null }

/** Complete identities only — a partial/legacy binding is ignored, never
 *  half-resolved (mirrors `resolvePickerProject`'s normalization). */
function normalizeBindingIdentity(b: BoardBindingIdentity | null): BoardProjectRef | null {
  if (!b || !b.projectOwner || b.projectNumber == null) {
    return null
  }
  return {
    owner: b.projectOwner,
    ownerType: b.projectOwnerType === 'organization' ? 'organization' : 'user',
    number: b.projectNumber
  }
}

function sameRef(a: BoardProjectRef, b: BoardProjectRef): boolean {
  return a.owner === b.owner && a.ownerType === b.ownerType && a.number === b.number
}

export function resolveBoardProject(input: {
  /** null = the standalone (non-hub) surface: pick map and binding are skipped. */
  repoId: string | null
  settings:
    | Pick<GitHubProjectSettings, 'activeProject' | 'activeProjectByRepo'>
    | null
    | undefined
  bindingState: BoardBindingState
}): BoardProjectResolution {
  const { repoId, settings, bindingState } = input
  if (repoId != null) {
    const pick = (settings?.activeProjectByRepo ?? {})[repoId] ?? null
    const bound =
      bindingState.status === 'loaded' ? normalizeBindingIdentity(bindingState.binding) : null
    if (pick) {
      // A pick needs no fetch — short-circuit even while the binding loads;
      // the divergence hint only fires once the binding is loaded + complete.
      const project = { owner: pick.owner, ownerType: pick.ownerType, number: pick.number }
      return {
        source: 'pick',
        project,
        divergesFromBinding: bound && !sameRef(bound, project) ? bound : null
      }
    }
    if (bindingState.status === 'loading') {
      return { source: 'pending', project: null }
    }
    if (bound) {
      return { source: 'binding', project: bound }
    }
  }
  const legacy = settings?.activeProject ?? null
  if (legacy) {
    return {
      source: 'legacy',
      project: { owner: legacy.owner, ownerType: legacy.ownerType, number: legacy.number }
    }
  }
  return { source: 'none', project: null }
}

/** ProjectPicker's commit, retargeted. repoId != null → write the pick to
 *  activeProjectByRepo[repoId]; recent + lastViewByProject stay GLOBAL
 *  (project-keyed, repo-agnostic); legacy activeProject is byte-untouched.
 *  repoId == null → the pre-016 standalone behavior verbatim (writes
 *  activeProject — the one surviving legacy write path, deliberately kept so
 *  the standalone board isn't left with a dead picker). */
export function applyBoardPick(
  prev: GitHubProjectSettings,
  repoId: string | null,
  selection: { owner: string; ownerType: GitHubProjectOwnerType; number: number; viewId?: string }
): GitHubProjectSettings {
  const key = `${selection.ownerType}:${selection.owner}:${selection.number}`
  const recent = [
    {
      owner: selection.owner,
      ownerType: selection.ownerType,
      number: selection.number,
      lastOpenedAt: new Date().toISOString()
    },
    ...prev.recent.filter((r) => `${r.ownerType}:${r.owner}:${r.number}` !== key)
  ].slice(0, 10)
  const lastViewByProject = { ...prev.lastViewByProject }
  if (selection.viewId) {
    lastViewByProject[key] = { viewId: selection.viewId }
  }
  const pick = { owner: selection.owner, ownerType: selection.ownerType, number: selection.number }
  if (repoId == null) {
    return { ...prev, recent, lastViewByProject, activeProject: pick }
  }
  return {
    ...prev,
    recent,
    lastViewByProject,
    // Spread-before-add tolerates upgraded profiles whose stored settings
    // object predates the map (the hydrate merge is top-level shallow).
    activeProjectByRepo: { ...(prev.activeProjectByRepo ?? {}), [repoId]: pick }
  }
}

/** The hint's one-click "Use bound project": delete the per-repo entry, all
 *  siblings (incl. legacy activeProject and every OTHER repo's entry) untouched. */
export function clearBoardPick(prev: GitHubProjectSettings, repoId: string): GitHubProjectSettings {
  const activeProjectByRepo = { ...(prev.activeProjectByRepo ?? {}) }
  delete activeProjectByRepo[repoId]
  return { ...prev, activeProjectByRepo }
}
