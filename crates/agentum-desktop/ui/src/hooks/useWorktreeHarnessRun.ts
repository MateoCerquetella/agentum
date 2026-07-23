// Spec 023: the IO shell over `lib/harness-run.ts` — resolve the engine run
// owning a worktree (matched by workdir, architecture Q1) and keep it fresh
// off the harness event stream. ONE `listHarnesses` on mount (never a poll —
// AC 4); afterwards every event for the matched run re-reads just its status,
// and while unmatched (a just-created run can register a beat before the
// workspace opens) any run-level event re-reads the list. The stream is the
// same auto-reconnecting WS every harness surface uses; closed on unmount.
import { useCallback, useEffect, useRef, useState } from 'react'
import {
  getHarnessStatus,
  listHarnesses,
  subscribeHarnessEvents,
  type HarnessEvent,
  type HarnessEventStream,
  type HarnessStatus
} from '@/runtime/harness-client'
import { findHarnessRunForWorkdir } from '@/lib/harness-run'
import { normalizeWorkdir } from '@/lib/workspace-harness-detect'

/** Instance-local confirmed snapshots plus the newest request for each
 * normalized workdir. Exported only so the IO-free ordering contract can be
 * regression tested without adding a DOM hook-test dependency. */
export type WorktreeHarnessSnapshotState = {
  snapshotsByWorkdir: ReadonlyMap<string, HarnessStatus>
  latestRequestByWorkdir: ReadonlyMap<string, number>
}

export function createWorktreeHarnessSnapshotState(): WorktreeHarnessSnapshotState {
  return {
    snapshotsByWorkdir: new Map(),
    latestRequestByWorkdir: new Map()
  }
}

export function beginWorktreeHarnessSnapshotRequest(
  state: WorktreeHarnessSnapshotState,
  workdir: string,
  requestId: number
): WorktreeHarnessSnapshotState {
  const normalized = normalizeWorkdir(workdir)
  const currentRequest = state.latestRequestByWorkdir.get(normalized) ?? 0
  if (requestId <= currentRequest) return state

  const latestRequestByWorkdir = new Map(state.latestRequestByWorkdir)
  latestRequestByWorkdir.set(normalized, requestId)
  return { ...state, latestRequestByWorkdir }
}

/** Resolve the newest authoritative request for a workdir. `undefined` is an
 * authoritative list miss and evicts that workdir. A foreign snapshot is
 * rejected rather than ever being exposed under the requested workdir. */
export function resolveWorktreeHarnessSnapshotRequest(
  state: WorktreeHarnessSnapshotState,
  workdir: string,
  requestId: number,
  run: HarnessStatus | undefined
): WorktreeHarnessSnapshotState {
  const normalized = normalizeWorkdir(workdir)
  if (state.latestRequestByWorkdir.get(normalized) !== requestId) return state

  if (run && !findHarnessRunForWorkdir([run], workdir)) return state

  const previous = state.snapshotsByWorkdir.get(normalized)
  if (run === previous || (!run && !previous)) return state

  const snapshotsByWorkdir = new Map(state.snapshotsByWorkdir)
  if (run) {
    snapshotsByWorkdir.set(normalized, run)
  } else {
    snapshotsByWorkdir.delete(normalized)
  }
  return { ...state, snapshotsByWorkdir }
}

export function selectWorktreeHarnessSnapshot(
  state: WorktreeHarnessSnapshotState,
  workdir: string | undefined
): HarnessStatus | undefined {
  if (!workdir) return undefined
  const snapshot = state.snapshotsByWorkdir.get(normalizeWorkdir(workdir))
  return snapshot ? findHarnessRunForWorkdir([snapshot], workdir) : undefined
}

export type WorktreeHarnessRun = {
  run: HarnessStatus | undefined
  /** Re-run the mount-time list read + match (e.g. after an unlink, so the
   *  chip clears deterministically alongside the engine's `log` event — AC 7).
   *  Bumps the effect, which re-reads and re-subscribes. */
  refresh: () => void
}

export function useWorktreeHarnessRun(workdir: string | undefined): WorktreeHarnessRun {
  const [snapshots, setSnapshots] = useState(createWorktreeHarnessSnapshotState)
  const nextRequestId = useRef(0)
  const [nonce, setNonce] = useState(0)
  const refresh = useCallback((): void => setNonce((n) => n + 1), [])
  const run = selectWorktreeHarnessSnapshot(snapshots, workdir)

  useEffect(() => {
    if (!workdir) return
    let disposed = false
    let stream: HarnessEventStream | null = null
    let matchedId: string | null = null
    let latestRequestId = 0

    const beginRequest = (): number => {
      const requestId = ++nextRequestId.current
      latestRequestId = requestId
      setSnapshots((current) =>
        beginWorktreeHarnessSnapshotRequest(current, workdir, requestId)
      )
      return requestId
    }

    const isCurrent = (requestId: number): boolean =>
      !disposed && requestId === latestRequestId

    const resolveRequest = (
      requestId: number,
      status: HarnessStatus | undefined
    ): void => {
      if (!isCurrent(requestId)) return
      setSnapshots((current) =>
        resolveWorktreeHarnessSnapshotRequest(current, workdir, requestId, status)
      )
    }

    const applyList = async (): Promise<void> => {
      const requestId = beginRequest()
      try {
        const runs = await listHarnesses()
        if (!isCurrent(requestId)) return
        const found = findHarnessRunForWorkdir(runs, workdir)
        matchedId = found?.id ?? null
        resolveRequest(requestId, found)
      } catch {
        // Best-effort: a failed read leaves the previous snapshot in place.
      }
    }
    const applyOne = async (id: string): Promise<void> => {
      const requestId = beginRequest()
      try {
        const status = await getHarnessStatus(id)
        if (!isCurrent(requestId)) return
        const owned = findHarnessRunForWorkdir([status], workdir)
        if (!owned) {
          // Ownership may have moved while the response was in flight. Re-list
          // before evicting so only an authoritative list miss clears the bar.
          await applyList()
          return
        }
        matchedId = owned.id
        resolveRequest(requestId, owned)
      } catch {
        // A 404 means the run was dropped from the engine — re-derive from
        // the authoritative list so the surface clears instead of going stale.
        if (isCurrent(requestId)) await applyList()
      }
    }
    const onEvent = (ev: HarnessEvent): void => {
      if (disposed) return
      if (ev.type === 'lagged') {
        void applyList()
        return
      }
      if (matchedId !== null && ev.harness_id === matchedId) {
        void applyOne(matchedId)
      } else if (matchedId === null) {
        // Not matched yet: any run-level event is worth one list re-read (the
        // run may have just registered). Event-driven, not a poll.
        void applyList()
      }
    }

    void applyList()
    subscribeHarnessEvents(onEvent)
      .then((s) => {
        if (disposed) {
          s.close()
        } else {
          stream = s
        }
      })
      .catch(() => {
        // No event stream → the mount-time snapshot stands; the surfaces
        // degrade to honest staleness rather than throwing or polling.
      })

    return () => {
      disposed = true
      stream?.close()
    }
  }, [workdir, nonce])

  return { run, refresh }
}
