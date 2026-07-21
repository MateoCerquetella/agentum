// Spec 023: the IO shell over `lib/harness-run.ts` — resolve the engine run
// owning a worktree (matched by workdir, architecture Q1) and keep it fresh
// off the harness event stream. ONE `listHarnesses` on mount (never a poll —
// AC 4); afterwards every event for the matched run re-reads just its status,
// and while unmatched (a just-created run can register a beat before the
// workspace opens) any run-level event re-reads the list. The stream is the
// same auto-reconnecting WS every harness surface uses; closed on unmount.
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
  /** Re-run the mount-time list read + match (e.g. after an unlink, so the
   *  chip clears deterministically alongside the engine's `log` event — AC 7).
   *  Bumps the effect, which re-reads and re-subscribes. */
  refresh: () => void
}

export function useWorktreeHarnessRun(workdir: string | undefined): WorktreeHarnessRun {
  const [run, setRun] = useState<HarnessStatus | undefined>(undefined)
  const [nonce, setNonce] = useState(0)
  const refresh = useCallback((): void => setNonce((n) => n + 1), [])

  useEffect(() => {
    if (!workdir) {
      setRun(undefined)
      return
    }
    let disposed = false
    let stream: HarnessEventStream | null = null
    let matchedId: string | null = null

    const applyList = async (): Promise<void> => {
      try {
        const runs = await listHarnesses()
        if (disposed) return
        const found = findHarnessRunForWorkdir(runs, workdir)
        matchedId = found?.id ?? null
        setRun(found)
      } catch {
        // Best-effort: a failed read leaves the previous snapshot in place.
      }
    }
    const applyOne = async (id: string): Promise<void> => {
      try {
        const status = await getHarnessStatus(id)
        if (!disposed) {
          setRun(status)
        }
      } catch {
        // A 404 means the run was dropped from the engine — re-derive from
        // the authoritative list so the surface clears instead of going stale.
        await applyList()
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
