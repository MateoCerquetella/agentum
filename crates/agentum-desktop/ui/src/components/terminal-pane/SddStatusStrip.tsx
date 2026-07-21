import { useEffect, useState } from 'react'
import { Workflow } from 'lucide-react'
import { getSession, type Session } from '@/runtime/agentum-server-client'
import {
  getHarnessStatus,
  listHarnesses,
  openHarnessEventStream,
  type HarnessEvent,
  type HarnessEventStream,
  type HarnessStatus,
  type SpecPhase
} from '@/runtime/harness-client'

const PHASE_LABELS: Record<SpecPhase, string> = {
  authoring: 'Authoring',
  architecture: 'Architecture',
  decompose: 'Decompose',
  executing: 'Executing',
  review: 'Review',
  done: 'Done',
  blocked: 'Blocked',
  awaiting_confirm: 'Awaiting confirmation'
}

type FeatureProgress = { current: number; total: number }

function harnessToken(id: string): string {
  return id.replaceAll('-', '').slice(0, 8).toLowerCase()
}

/** Resolve all harness-created sessions, including completed role/QA sessions.
 * `current_session` alone cannot do that because it is cleared between gates. */
export function resolveHarnessForSession(
  session: Pick<Session, 'id' | 'name' | 'workdir'>,
  harnesses: HarnessStatus[]
): HarnessStatus | null {
  const byCurrentSession = harnesses.find((harness) => harness.current_session === session.id)
  if (byCurrentSession) return byCurrentSession

  if (!session.name.startsWith('harness-')) return null
  const suffix = session.name.match(/-([0-9a-f]{8})$/i)?.[1]?.toLowerCase()
  if (!suffix) return null
  return (
    harnesses.find(
      (harness) => harness.workdir === session.workdir && harnessToken(harness.id) === suffix
    ) ?? null
  )
}

export function getFeatureProgress(status: HarnessStatus): FeatureProgress | null {
  if (status.phase !== 'executing' || !status.current_feature) return null
  const index = status.features.features.findIndex(
    (feature) => feature.id === status.current_feature
  )
  return index < 0 ? null : { current: index + 1, total: status.features.features.length }
}

function eventBelongsToHarness(event: HarnessEvent, harnessId: string): boolean {
  return event.type === 'lagged' || event.harness_id === harnessId
}

export function SddStatusStripView({ status }: { status: HarnessStatus }): React.JSX.Element {
  const phase = status.phase ?? 'executing'
  const progress = getFeatureProgress(status)

  return (
    <div
      className="pointer-events-none absolute inset-x-0 top-0 z-20 flex h-7 items-center justify-center border-b border-border/70 bg-background/95 px-3 text-xs text-muted-foreground shadow-sm backdrop-blur"
      data-sdd-status-strip
      role="status"
      aria-label={`SDD run status: ${PHASE_LABELS[phase]}${progress ? `, feature ${progress.current} of ${progress.total}` : ''}`}
    >
      <div className="flex min-w-0 items-center gap-2">
        <Workflow aria-hidden="true" className="size-3.5 shrink-0 text-primary" />
        <span className="font-medium text-foreground">SDD</span>
        <span aria-hidden="true" className="text-border">/</span>
        <span>{PHASE_LABELS[phase]}</span>
        {progress && (
          <>
            <span aria-hidden="true" className="text-border">/</span>
            <span className="font-mono tabular-nums text-foreground">
              {progress.current}/{progress.total}
            </span>
          </>
        )}
      </div>
    </div>
  )
}

export function SddStatusStrip({
  serverSessionId,
  visible
}: {
  serverSessionId?: string
  visible: boolean
}): React.JSX.Element | null {
  const [status, setStatus] = useState<HarnessStatus | null>(null)

  useEffect(() => {
    setStatus(null)
    if (!serverSessionId) return

    let disposed = false
    let stream: HarnessEventStream | null = null

    const connect = async (): Promise<void> => {
      try {
        const [session, harnesses] = await Promise.all([
          getSession(serverSessionId),
          listHarnesses()
        ])
        if (disposed) return
        const harness = resolveHarnessForSession(session, harnesses)
        if (!harness) return
        setStatus(harness)

        let refreshPending = false
        let refreshQueued = false
        const refresh = (): void => {
          if (refreshPending) {
            refreshQueued = true
            return
          }
          refreshPending = true
          void getHarnessStatus(harness.id)
            .then((next) => {
              if (!disposed) setStatus(next)
            })
            .catch(() => undefined)
            .finally(() => {
              refreshPending = false
              if (refreshQueued && !disposed) {
                refreshQueued = false
                refresh()
              }
            })
        }
        stream = await openHarnessEventStream((event) => {
          if (disposed || !eventBelongsToHarness(event, harness.id)) return
          refresh()
        })
        if (disposed) stream.close()
      } catch {
        // A plain/deleted session or unavailable harness service has no strip.
      }
    }

    void connect()
    return () => {
      disposed = true
      stream?.close()
    }
  }, [serverSessionId])

  return visible && status ? <SddStatusStripView status={status} /> : null
}
