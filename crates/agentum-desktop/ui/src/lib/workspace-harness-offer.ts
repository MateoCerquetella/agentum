// Spec 015: the async IO shell around `lib/workspace-harness-detect.ts` — runs
// once per workspace creation (fired fire-and-forget from
// `openCreatedWorkspace`, D2) and writes the offer slice only on a positive
// detection. Any failure means "no banner", never a broken create flow.
import { toast } from 'sonner'
import { useAppStore } from '@/store'
import { fsListEntries, type FsFileEntry } from '@/runtime/server-fs-client'
import {
  listHarnesses,
  runHarness,
  startHarness,
  subscribeHarnessRunErrors
} from '@/runtime/harness-client'
import {
  HARNESS_DIR,
  LEGACY_HARNESS_DIR,
  decideHarnessOffer,
  detectHarnessSpec,
  normalizeWorkdir,
  shouldDetectHarnessSpec,
  type WorkspaceHarnessOffer
} from './workspace-harness-detect'

/**
 * List a directory's entries, folding EVERY failure to `null`. A missing dir
 * is a `BadRequest("path error: …")` from `routes/fs.rs`, and a transient
 * server error changes nothing about the outcome (no banner, fail-closed) —
 * distinguishing them buys this flow nothing. SSH worktrees pass their
 * resolved server host id; local worktrees omit it.
 */
async function listOrNull(path: string, hostId?: string): Promise<FsFileEntry[] | null> {
  try {
    const listing = await fsListEntries(path, { hidden: true, ...(hostId ? { hostId } : {}) })
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
    // Spec 023 Part A: same purge for the gated-run-starting slice.
    state.clearGatedRunStarting(opts.worktreeId)

    // Resolve worktree + connectionId from the store (the
    // WorkspaceAgentLauncher pattern). `undefined` = not found → fail closed.
    const worktree = Object.values(state.worktreesByRepo ?? {})
      .flat()
      .find((w) => w.id === opts.worktreeId)
    if (!worktree) {
      return
    }

    // Spec 023 Part A (AC 1): a gated-run creation — the composer passes
    // `gatedRun` ONLY when the engine took ownership
    // (`gatedRunResultOwnsWorktree` → true) — surfaces the starting run
    // instead of the bare picker. Set SYNCHRONOUSLY (before any await below)
    // so the workspace never flashes the launcher; the offer detection is
    // skipped for gated runs anyway (`shouldDetectHarnessSpec` D6), and the
    // `subscribeHarnessRunErrors` toast still covers a mid-spawn failure
    // (AC 3). Cleared by the GatedRunSurface once the session is attachable.
    if (opts.gatedRun) {
      state.setGatedRunStarting({ worktreeId: opts.worktreeId, workdir: worktree.path })
      return
    }

    const repo = state.repos?.find((candidate) => candidate.id === worktree.repoId)
    if (!repo) return
    const connectionId = repo.connectionId ?? null
    if (!shouldDetectHarnessSpec({ gatedRun: false, connectionId })) {
      return
    }
    // A known SSH repo without its server host mapping is not safe to query.
    // Repo hydration normally fills hostId; fail closed during that brief gap.
    const hostId = connectionId ? (repo?.hostId ?? undefined) : undefined
    if (connectionId && !hostId) return

    const base = normalizeWorkdir(worktree.path)
    const canonical = await listOrNull(`${base}/${HARNESS_DIR}`, hostId)
    // Legacy is consulted ONLY when the canonical dir is absent — the fold in
    // detectHarnessSpec mirrors the server's resolve_harness_dir, and skipping
    // the second fetch keeps the not-found path at ≤2 fs calls (AC 6).
    const legacy = canonical === null
      ? await listOrNull(`${base}/${LEGACY_HARNESS_DIR}`, hostId)
      : null
    const detection = detectHarnessSpec(canonical, legacy)
    if (!detection.found) {
      return
    }

    // AC 5 dedupe — only reached on the found path, so the not-found flow
    // stays at the fs listing alone (AC 6: no other network).
    const registeredRuns = await listHarnesses()
    const offer = decideHarnessOffer({
      detection,
      worktreeId: opts.worktreeId,
      workdir: worktree.path,
      allowLegacyLocalPathFallback: !connectionId,
      registeredRuns
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

/**
 * Accept the offer: register the project with the engine, kick off the drive
 * loop, and surface early drive-phase failures. The gate is sacred — this is
 * `POST /api/harness` + `POST /{id}/run` and NOTHING else (the engine spawns
 * the agents; init/verify semantics are untouched).
 *
 * On failure the slice entry is KEPT (the banner stays mounted, retryable)
 * and the toast carries the server's error detail — the harness-client
 * `request()` helper already embeds the response text in `error.message`.
 *
 * Deviation from architecture §5 (documented in tasks.md): the failure is
 * toasted HERE and swallowed rather than re-thrown — the resolved promise is
 * the component's "settle" signal and its only job is the busy flag, so no
 * caller needs a try/catch and nothing can become an unhandled rejection.
 */
export async function acceptHarnessOffer(offer: WorkspaceHarnessOffer): Promise<void> {
  try {
    const { harness_id } = await startHarness({
      workdir: offer.workdir,
      worktreeId: offer.worktreeId
    })
    await runHarness(harness_id)
    toast.success('Harness run started')
    useAppStore.getState().clearWorkspaceHarnessOffer(offer.worktreeId)
    // runHarness returns before the bg drive loop does anything, so the most
    // common failure class — a red init.sh seconds later — would otherwise
    // vanish. Bounded, self-closing subscription (spec 008 F1 precedent).
    void subscribeHarnessRunErrors(harness_id, (message) => {
      toast.error(`Harness run failed: ${message}`)
    })
  } catch (error) {
    // Swallow after toasting: the slice entry stays (banner remains mounted,
    // retryable) and the caller only needs to reset its busy flag.
    toast.error(error instanceof Error ? error.message : String(error))
  }
}
