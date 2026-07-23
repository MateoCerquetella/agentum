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
  /** Re-run the mount-time list read + match (e.g. after retry or unlink). */
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
        // Failed reconciliation retains the last successful snapshot for this
        // workdir. A later connection/event/explicit refresh can reconcile it.
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
  }, [workdir, nonce])

  return { run, refresh }
}
