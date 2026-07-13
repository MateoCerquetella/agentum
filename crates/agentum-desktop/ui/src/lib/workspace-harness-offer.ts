// Spec 015: the async IO shell around `lib/workspace-harness-detect.ts` — runs
// once per workspace creation (fired fire-and-forget from
// `openCreatedWorkspace`, D2) and writes the offer slice only on a positive
// detection. Any failure means "no banner", never a broken create flow.
import { useAppStore } from '@/store'
import { fsListEntries, type FsFileEntry } from '@/runtime/server-fs-client'
import {
  HARNESS_DIR,
  LEGACY_HARNESS_DIR,
  decideHarnessOffer,
  detectHarnessSpec,
  normalizeWorkdir,
  shouldDetectHarnessSpec
} from './workspace-harness-detect'

/**
 * List a directory's entries, folding EVERY failure to `null`. A missing dir
 * is a `BadRequest("path error: …")` from `routes/fs.rs`, and a transient
 * server error changes nothing about the outcome (no banner, fail-closed) —
 * distinguishing them buys v1 nothing. `hostId` is never passed: D5 guarantees
 * the workdir is local before any fs call.
 */
async function listOrNull(path: string): Promise<FsFileEntry[] | null> {
  try {
    const listing = await fsListEntries(path, { hidden: true })
    return listing.entries
  } catch {
    return null
  }
}

/**
 * Detect a harness spec in a just-created workspace's workdir and, when one
 * is found (and the workdir isn't already registered with the engine), set the
 * per-worktree offer the `HarnessSpecBanner` renders. Fire-and-forget: the
 * caller `void`s the promise; one outer catch swallows everything.
 */
export async function maybeOfferWorkspaceHarnessRun(opts: {
  worktreeId: string
  gatedRun: boolean
}): Promise<void> {
  try {
    const state = useAppStore.getState()
    // Stale purge FIRST: worktree ids are `${repoId}::${path}`, so a
    // close-then-recreate at the same path reuses the id — without this, a
    // pre-close offer could leak into a new gated creation (D6 violation).
    state.clearWorkspaceHarnessOffer(opts.worktreeId)

    // Resolve worktree + connectionId from the store (the
    // WorkspaceAgentLauncher pattern). `undefined` = not found → fail closed.
    const worktree = Object.values(state.worktreesByRepo ?? {})
      .flat()
      .find((w) => w.id === opts.worktreeId)
    const connectionId = worktree
      ? (state.repos?.find((r) => r.id === worktree.repoId)?.connectionId ?? null)
      : undefined
    if (!worktree || !shouldDetectHarnessSpec({ gatedRun: opts.gatedRun, connectionId })) {
      return
    }

    const base = normalizeWorkdir(worktree.path)
    const canonical = await listOrNull(`${base}/${HARNESS_DIR}`)
    // Legacy is consulted ONLY when the canonical dir is absent — the fold in
    // detectHarnessSpec mirrors the server's resolve_harness_dir, and skipping
    // the second fetch keeps the not-found path at ≤2 fs calls (AC 6).
    const legacy = canonical === null ? await listOrNull(`${base}/${LEGACY_HARNESS_DIR}`) : null
    const detection = detectHarnessSpec(canonical, legacy)
    if (!detection.found) {
      return
    }

    // f3 wires listHarnesses() here for the AC 5 dedupe; until then nothing is
    // ever considered pre-registered.
    const registeredWorkdirs: string[] = []
    const offer = decideHarnessOffer({
      detection,
      worktreeId: opts.worktreeId,
      workdir: worktree.path,
      registeredWorkdirs
    })
    if (!offer) {
      return
    }

    // Close-race re-check: the workspace may have been closed while detection
    // was in flight — never surface an offer for a gone worktree.
    const stillPresent = Object.values(useAppStore.getState().worktreesByRepo ?? {})
      .flat()
      .some((w) => w.id === opts.worktreeId)
    if (!stillPresent) {
      return
    }
    useAppStore.getState().setWorkspaceHarnessOffer(offer)
  } catch {
    // Fire-and-forget: a failed detection is a skipped offer, nothing more.
  }
}
