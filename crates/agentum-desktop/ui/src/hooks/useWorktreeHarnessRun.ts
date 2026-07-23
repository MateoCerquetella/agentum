// Spec 023: the IO shell over `lib/harness-run.ts` — resolve the engine run
// owning a worktree (matched by workdir, architecture Q1) and keep it fresh
// from authoritative list/status snapshots. Reads are event-driven rather than
// polled: mount, successful stream connections, lagged frames, matched run
// events, and the explicit refresh contract.
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
 * authoritative worktree id (or normalized legacy path). Exported only so the
 * IO-free ordering contract can be regression tested. */
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

function worktreeSnapshotKey(workdir: string, worktreeId?: string): string {
  return worktreeId ? `id:${worktreeId}` : `path:${normalizeWorkdir(workdir)}`
}

export function beginWorktreeHarnessSnapshotRequest(
  state: WorktreeHarnessSnapshotState,
  workdir: string,
  requestId: number,
  worktreeId?: string
): WorktreeHarnessSnapshotState {
  const key = worktreeSnapshotKey(workdir, worktreeId)
  const currentRequest = state.latestRequestByWorkdir.get(key) ?? 0
  if (requestId <= currentRequest) return state

  const latestRequestByWorkdir = new Map(state.latestRequestByWorkdir)
  latestRequestByWorkdir.set(key, requestId)
  return { ...state, latestRequestByWorkdir }
}

/** Resolve the newest authoritative request for a workdir. `undefined` is an
 * authoritative list miss and evicts that workdir. A foreign snapshot is
 * rejected rather than ever being exposed under the requested workdir. */
export function resolveWorktreeHarnessSnapshotRequest(
  state: WorktreeHarnessSnapshotState,
  workdir: string,
  requestId: number,
  run: HarnessStatus | undefined,
  worktreeId: string | undefined,
  allowLegacyLocalPathFallback: boolean
): WorktreeHarnessSnapshotState {
  const key = worktreeSnapshotKey(workdir, worktreeId)
  if (state.latestRequestByWorkdir.get(key) !== requestId) return state

  if (
    run &&
    !findHarnessRunForWorkdir(
      [run],
      workdir,
      worktreeId,
      allowLegacyLocalPathFallback
    )
  ) {
    return state
  }

  const previous = state.snapshotsByWorkdir.get(key)
  if (run === previous || (!run && !previous)) return state

  const snapshotsByWorkdir = new Map(state.snapshotsByWorkdir)
  if (run) {
    snapshotsByWorkdir.set(key, run)
  } else {
    snapshotsByWorkdir.delete(key)
  }
  return { ...state, snapshotsByWorkdir }
}

export function selectWorktreeHarnessSnapshot(
  state: WorktreeHarnessSnapshotState,
  workdir: string | undefined,
  worktreeId: string | undefined,
  allowLegacyLocalPathFallback: boolean
): HarnessStatus | undefined {
  if (!workdir) return undefined
  const snapshot = state.snapshotsByWorkdir.get(worktreeSnapshotKey(workdir, worktreeId))
  return snapshot
    ? findHarnessRunForWorkdir(
        [snapshot],
        workdir,
        worktreeId,
        allowLegacyLocalPathFallback
      )
    : undefined
}

export type WorktreeHarnessRun = {
  run: HarnessStatus | undefined
  /** Re-run the mount-time list read + match (e.g. after retry or unlink). */
  refresh: () => void
}

export function useWorktreeHarnessRun(
  workdir: string | undefined,
  worktreeId: string | undefined,
  allowLegacyLocalPathFallback: boolean
): WorktreeHarnessRun {
  const [snapshots, setSnapshots] = useState(createWorktreeHarnessSnapshotState)
  const nextRequestId = useRef(0)
  const [nonce, setNonce] = useState(0)
  const refresh = useCallback((): void => setNonce((n) => n + 1), [])
  const run = selectWorktreeHarnessSnapshot(
    snapshots,
    workdir,
    worktreeId,
    allowLegacyLocalPathFallback
  )

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
        beginWorktreeHarnessSnapshotRequest(current, workdir, requestId, worktreeId)
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
        resolveWorktreeHarnessSnapshotRequest(
          current,
          workdir,
          requestId,
          status,
          worktreeId,
          allowLegacyLocalPathFallback
        )
      )
    }

    const applyList = async (): Promise<void> => {
      const requestId = beginRequest()
      try {
        const runs = await listHarnesses()
        if (!isCurrent(requestId)) return
        const found = findHarnessRunForWorkdir(
          runs,
          workdir,
          worktreeId,
          allowLegacyLocalPathFallback
        )
        matchedId = found?.id ?? null
        resolveRequest(requestId, found)
      } catch {
        // Failed reconciliation retains the last successful snapshot for this
        // workdir. A later connection/event/explicit refresh can reconcile it.
      }
    }

    const applyOne = async (id: string): Promise<void> => {
      const requestId = beginRequest()
      try {
        const status = await getHarnessStatus(id)
        if (!isCurrent(requestId)) return
        const owned = findHarnessRunForWorkdir(
          [status],
          workdir,
          worktreeId,
          allowLegacyLocalPathFallback
        )
        if (!owned) {
          // Ownership may have moved while the response was in flight. Re-list
          // before evicting so only an authoritative list miss clears the bar.
          await applyList()
          return
        }
        matchedId = owned.id
        resolveRequest(requestId, owned)
      } catch {
        // The run may have disappeared. Re-read membership so a successful
        // no-match clears the bar, while a failed list read preserves it.
        if (isCurrent(requestId)) await applyList()
      }
    }

    const onEvent = (event: HarnessEvent): void => {
      if (disposed) return
      if (event.type === 'lagged') {
        void applyList()
        return
      }
      if (matchedId !== null && event.harness_id === matchedId) {
        void applyOne(matchedId)
      } else if (matchedId === null) {
        // A run can register just before its first event reaches this client.
        void applyList()
      }
    }

    const onConnected = (): void => {
      if (!disposed) void applyList()
    }

    void applyList()
    subscribeHarnessEvents(onEvent, onConnected)
      .then((subscribedStream) => {
        if (disposed) {
          subscribedStream.close()
        } else {
          stream = subscribedStream
        }
      })
      .catch(() => {
        // The current snapshot remains useful if the stream is unavailable.
      })

    return () => {
      disposed = true
      stream?.close()
    }
  }, [workdir, worktreeId, allowLegacyLocalPathFallback, nonce])

  return { run, refresh }
}
