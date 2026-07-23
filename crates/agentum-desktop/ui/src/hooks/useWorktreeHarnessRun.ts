// Spec 023: the IO shell over `lib/harness-run.ts` — resolve the engine run
// owning a worktree (matched by workdir, architecture Q1) and keep it fresh
// from authoritative list/status snapshots. Reads are event-driven rather than
// polled: mount, successful stream connections, lagged frames, matched run
// events, and the explicit refresh contract.
import { useCallback, useEffect, useState } from 'react'
import {
  getHarnessStatus,
  listHarnesses,
  subscribeHarnessEvents,
  type HarnessEvent,
  type HarnessEventStream,
  type HarnessStatus
} from '@/runtime/harness-client'
import { findHarnessRunForWorkdir } from '@/lib/harness-run'

export type WorktreeHarnessRun = {
  run: HarnessStatus | undefined
  /** Re-run the mount-time list read + match (e.g. after retry or unlink). */
  refresh: () => void
}

type WorktreeRunSnapshot = {
  workdir: string
  run: HarnessStatus | undefined
}

export function useWorktreeHarnessRun(workdir: string | undefined): WorktreeHarnessRun {
  const [snapshot, setSnapshot] = useState<WorktreeRunSnapshot | undefined>(undefined)
  const [nonce, setNonce] = useState(0)
  const refresh = useCallback((): void => setNonce((n) => n + 1), [])

  useEffect(() => {
    if (!workdir) {
      setSnapshot(undefined)
      return
    }

    let disposed = false
    let stream: HarnessEventStream | null = null
    let matchedId: string | null = null
    let requestGeneration = 0

    const applyList = async (): Promise<void> => {
      const generation = ++requestGeneration
      try {
        const runs = await listHarnesses()
        if (disposed || generation !== requestGeneration) return
        const found = findHarnessRunForWorkdir(runs, workdir)
        matchedId = found?.id ?? null
        setSnapshot({ workdir, run: found })
      } catch {
        // Failed reconciliation retains the last successful snapshot for this
        // workdir. A later connection/event/explicit refresh can reconcile it.
      }
    }

    const applyOne = async (id: string): Promise<void> => {
      const generation = ++requestGeneration
      try {
        const status = await getHarnessStatus(id)
        if (disposed || generation !== requestGeneration) return
        matchedId = status.id
        setSnapshot({ workdir, run: status })
      } catch {
        // The run may have disappeared. Re-read membership so a successful
        // no-match clears the bar, while a failed list read preserves it.
        await applyList()
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
      requestGeneration += 1
      stream?.close()
    }
  }, [workdir, nonce])

  return {
    run: snapshot?.workdir === workdir ? snapshot.run : undefined,
    refresh
  }
}
