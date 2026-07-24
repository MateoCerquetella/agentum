// Pure, IO-free detection model for spec 015 (workspace harness autostart):
// given a just-created workspace's fs entries, the creation context, and the
// already-registered harness workdirs, decide whether to offer a one-click
// "Start Harness run". No runtime/store imports — the async shell lives in
// `lib/workspace-harness-offer.ts` (model: `lib/workspace-goal-step.ts`).
import type { FsFileEntry } from '@/runtime/server-fs-client'

/** Canonical per-project harness directory — mirrors the server's
 *  `HARNESS_DIR` (`crates/agentum-server/src/harness/types.rs:16`). Never
 *  invent a third spelling. */
export const HARNESS_DIR = '.agentum-harness'
/** Pre-010 directory name — mirrors `LEGACY_HARNESS_DIR`
 *  (`harness/types.rs:19`). Read only when the canonical dir is absent. */
export const LEGACY_HARNESS_DIR = '.harness'
export const FEATURE_LIST_FILE = 'feature_list.json'

export type HarnessDirName = typeof HARNESS_DIR | typeof LEGACY_HARNESS_DIR

/**
 * Mirror of the server's `expand_workdir` normalization
 * (`routes/util.rs:24-42`): trim; strip trailing `/` unless the whole path is
 * `/`. No symlink canonicalization — neither does the server. `~` never
 * appears in `worktree.path` and comes back pre-expanded from the server, so
 * tilde expansion is deliberately NOT mirrored here.
 */
export function normalizeWorkdir(path: string): string {
  const trimmed = path.trim()
  return trimmed.length > 1 ? trimmed.replace(/\/+$/, '') : trimmed
}

/**
 * Pre-fs gate: a gated-run creation already registers via start-work, while an
 * unknown worktree still fails closed. Known local and SSH worktrees are both
 * detectable because the listing and Harness registration routes are host-aware.
 */
export function shouldDetectHarnessSpec(ctx: {
  gatedRun: boolean
  connectionId: string | null | undefined
}): boolean {
  if (ctx.gatedRun) {
    return false
  }
  return ctx.connectionId !== undefined
}

/**
 * Whether a listing contains the spec file itself. Kind must be `file`: a
 * DIRECTORY named `feature_list.json` is not a spec. The server's fs route
 * follows symlinks for `kind` (`routes/fs.rs:236-247`), so a symlinked spec
 * file still counts.
 */
export function hasFeatureList(entries: FsFileEntry[]): boolean {
  return entries.some((e) => e.name === FEATURE_LIST_FILE && e.kind === 'file')
}

export type HarnessSpecDetection = { found: false } | { found: true; harnessDir: HarnessDirName }

/**
 * Fold the two directory listings into a detection verdict (`null` = dir
 * missing/unlistable). CRITICAL semantics: mirrors `resolve_harness_dir`
 * (`harness/types.rs:25-35`) — if the CANONICAL dir exists (its listing
 * succeeded), decide from it ALONE; the legacy dir is only consulted when the
 * canonical dir is absent. Otherwise the banner could offer a legacy run that
 * the engine (which prefers an existing `.agentum-harness/`) cannot load.
 */
export function detectHarnessSpec(
  canonicalEntries: FsFileEntry[] | null,
  legacyEntries: FsFileEntry[] | null
): HarnessSpecDetection {
  if (canonicalEntries !== null) {
    return hasFeatureList(canonicalEntries) ? { found: true, harnessDir: HARNESS_DIR } : { found: false }
  }
  if (legacyEntries !== null && hasFeatureList(legacyEntries)) {
    return { found: true, harnessDir: LEGACY_HARNESS_DIR }
  }
  return { found: false }
}

export type WorkspaceHarnessOffer = {
  worktreeId: string
  /** `worktree.path`, un-normalized — exactly what we POST to `/api/harness`
   *  (the server runs its own `expand_workdir`). */
  workdir: string
  harnessDir: HarnessDirName
}

/**
 * Dedupe + the final offer. Scoped runs match worktree id; normalized paths are
 * used only for legacy statuses that predate authoritative run identity.
 */
export function decideHarnessOffer(input: {
  detection: HarnessSpecDetection
  worktreeId: string
  workdir: string
  /** Legacy statuses have no worktree/host scope. Only a local target may
   *  safely dedupe against one by path; SSH paths are not globally unique. */
  allowLegacyLocalPathFallback: boolean
  registeredRuns?: Array<{ workdir: string; worktree_id?: string | null }>
  /** Backward-compatible pure-model input for pre-scope callers/tests. */
  registeredWorkdirs?: string[]
}): WorkspaceHarnessOffer | null {
  if (!input.detection.found) {
    return null
  }
  const normalized = normalizeWorkdir(input.workdir)
  const registered = input.registeredRuns ?? []
  const alreadyRegistered = registered.some(
    (run) =>
      run.worktree_id === input.worktreeId ||
      (input.allowLegacyLocalPathFallback &&
        !run.worktree_id &&
        normalizeWorkdir(run.workdir) === normalized)
  )
  const legacyRegistered =
    input.allowLegacyLocalPathFallback &&
    (input.registeredWorkdirs ?? []).some(
      (workdir) => normalizeWorkdir(workdir) === normalized
    )
  if (alreadyRegistered || legacyRegistered) {
    return null
  }
  return {
    worktreeId: input.worktreeId,
    workdir: input.workdir,
    harnessDir: input.detection.harnessDir
  }
}
