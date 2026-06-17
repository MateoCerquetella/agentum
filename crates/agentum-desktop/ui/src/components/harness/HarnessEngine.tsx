// Harness Engine — a real feature surface (not a demo). Drives agents one
// feature at a time behind a verification gate, against the embedded
// agentum-server `/api/harness/*` routes. Live state arrives over the
// `WS /api/harness/events` stream; the board + gate + file viewer reflect it.
import React from 'react'
import {
  AlertTriangle,
  CheckCircle2,
  ChevronLeft,
  CircleDot,
  FileText,
  FolderOpen,
  Loader2,
  Play,
  RefreshCw,
  ShieldCheck,
  ShieldX,
  Square,
  Terminal,
  XCircle
} from 'lucide-react'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import {
  getHarnessFiles,
  getHarnessStatus,
  listHarnesses,
  openHarnessEventStream,
  runHarness,
  startHarness,
  stopHarness,
  type Feature,
  type FeatureState,
  type HarnessEvent,
  type HarnessFiles,
  type HarnessState,
  type HarnessStatus
} from '@/runtime/harness-client'

type LogLine = { id: number; text: string; tone: 'info' | 'good' | 'bad' | 'warn' }

// One timestamped, human-readable line per inbound event for the activity log.
function describeEvent(ev: HarnessEvent): LogLine | null {
  switch (ev.type) {
    case 'state_changed':
      return { id: 0, tone: 'info', text: `run state → ${ev.state}` }
    case 'feature_state_changed':
      return {
        id: 0,
        tone: ev.state === 'done' ? 'good' : ev.state === 'blocked' ? 'bad' : 'info',
        text: `feature ${ev.feature_id} → ${ev.state}`
      }
    case 'init_started':
      return { id: 0, tone: 'info', text: 'init.sh started' }
    case 'init_completed':
      return {
        id: 0,
        tone: ev.success ? 'good' : 'bad',
        text: `init.sh ${ev.success ? 'passed' : 'FAILED'}`
      }
    case 'agent_spawned':
      return { id: 0, tone: 'info', text: `agent spawned for ${ev.feature_id}` }
    case 'log':
      return { id: 0, tone: 'info', text: ev.message }
    case 'verify_started':
      return { id: 0, tone: 'warn', text: `verify.sh running for ${ev.feature_id}` }
    case 'verify_completed':
      return {
        id: 0,
        tone: ev.success ? 'good' : 'bad',
        text: `verify ${ev.feature_id}: ${ev.success ? 'PASSED ✓' : 'FAILED ✗'}`
      }
    case 'handoff_written':
      return { id: 0, tone: 'good', text: `handoff.md written for ${ev.feature_id}` }
    case 'harness_completed':
      return {
        id: 0,
        tone: ev.success ? 'good' : 'bad',
        text: ev.success ? 'harness completed — all features verified' : 'harness stopped'
      }
    case 'error':
      return { id: 0, tone: 'bad', text: `error: ${ev.message}` }
    case 'lagged':
      return { id: 0, tone: 'warn', text: `event stream lagged (${ev.skipped} skipped)` }
    default:
      return null
  }
}

// Live harness state: fetch status + files, then keep them fresh from the event
// stream (with a slow polling fallback while a run is active).
function useHarnessLive(harnessId: string | null) {
  const [status, setStatus] = React.useState<HarnessStatus | null>(null)
  const [files, setFiles] = React.useState<HarnessFiles | null>(null)
  const [log, setLog] = React.useState<LogLine[]>([])
  const logSeq = React.useRef(0)

  const refreshStatus = React.useCallback(async () => {
    if (!harnessId) return
    try {
      setStatus(await getHarnessStatus(harnessId))
    } catch {
      /* transient — the next event or poll re-fetches */
    }
  }, [harnessId])

  const refreshFiles = React.useCallback(async () => {
    if (!harnessId) return
    try {
      setFiles(await getHarnessFiles(harnessId))
    } catch {
      /* ignore */
    }
  }, [harnessId])

  React.useEffect(() => {
    setStatus(null)
    setFiles(null)
    setLog([])
    if (!harnessId) return
    void refreshStatus()
    void refreshFiles()
  }, [harnessId, refreshStatus, refreshFiles])

  // Event stream → append to log + refetch the surfaces that changed.
  React.useEffect(() => {
    if (!harnessId) return
    let stream: { close: () => void } | null = null
    let cancelled = false
    void openHarnessEventStream((ev) => {
      const harnessMatches = 'harness_id' in ev ? ev.harness_id === harnessId : true
      if (!harnessMatches) return
      const line = describeEvent(ev)
      if (line) {
        setLog((prev) => {
          logSeq.current += 1
          const next = [...prev, { ...line, id: logSeq.current }]
          return next.length > 200 ? next.slice(next.length - 200) : next
        })
      }
      void refreshStatus()
      if (
        ev.type === 'handoff_written' ||
        ev.type === 'verify_completed' ||
        ev.type === 'feature_state_changed'
      ) {
        void refreshFiles()
      }
    }).then((s) => {
      if (cancelled) {
        s.close()
      } else {
        stream = s
      }
    })
    return () => {
      cancelled = true
      stream?.close()
    }
  }, [harnessId, refreshStatus, refreshFiles])

  // Polling fallback while a run is in flight (covers any missed event).
  React.useEffect(() => {
    if (!harnessId) return
    const active =
      status?.state === 'running' ||
      status?.state === 'verifying' ||
      status?.state === 'init_verifying'
    if (!active) return
    const t = setInterval(() => void refreshStatus(), 3000)
    return () => clearInterval(t)
  }, [harnessId, status?.state, refreshStatus])

  return { status, files, log, refreshStatus, refreshFiles }
}

// ---- Verification gate banner ---------------------------------------------

type GateTone = 'idle' | 'working' | 'verifying' | 'passed' | 'blocked' | 'failed' | 'done'

function deriveGate(status: HarnessStatus | null): { tone: GateTone; title: string; detail: string } {
  if (!status) return { tone: 'idle', title: 'No harness loaded', detail: 'Load a project with a .harness/ folder to begin.' }
  const blocked = status.features.features.find((f) => f.state === 'blocked')
  switch (status.state) {
    case 'done':
      return { tone: 'done', title: 'ALL FEATURES VERIFIED', detail: 'Every feature passed the gate.' }
    case 'failed':
      return { tone: 'failed', title: 'RUN FAILED', detail: 'init.sh failed or an orchestration error halted the run.' }
    case 'blocked':
      return {
        tone: 'blocked',
        title: 'VERIFICATION GATE: BLOCKED',
        detail: blocked
          ? `“${blocked.name}” exhausted ${blocked.attempts} attempt(s). Advancement is halted.`
          : 'A feature exhausted its retries.'
      }
    case 'verifying':
    case 'init_verifying':
      return { tone: 'verifying', title: 'VERIFYING…', detail: 'Running the gate. Advancement is blocked until it passes.' }
    case 'running':
      return { tone: 'working', title: 'AGENT WORKING', detail: 'An agent is implementing the current feature.' }
    case 'idle':
    default:
      return { tone: 'idle', title: 'READY', detail: 'Press Run to drive the harness.' }
  }
}

const GATE_STYLES: Record<GateTone, { wrap: string; icon: React.ReactNode }> = {
  idle: { wrap: 'border-border bg-card text-muted-foreground', icon: <CircleDot className="size-7" /> },
  working: {
    wrap: 'border-[var(--chart-3)]/40 bg-[var(--chart-3)]/10 text-[var(--chart-3)]',
    icon: <Loader2 className="size-7 animate-spin" />
  },
  verifying: {
    wrap: 'border-[var(--chart-3)]/50 bg-[var(--chart-3)]/12 text-[var(--chart-3)]',
    icon: <Loader2 className="size-7 animate-spin" />
  },
  passed: {
    wrap: 'border-[var(--chart-1)]/50 bg-[var(--chart-1)]/12 text-[var(--chart-1)]',
    icon: <ShieldCheck className="size-7" />
  },
  done: {
    wrap: 'border-[var(--chart-1)]/50 bg-[var(--chart-1)]/12 text-[var(--chart-1)]',
    icon: <CheckCircle2 className="size-7" />
  },
  blocked: {
    wrap: 'border-destructive/50 bg-destructive/12 text-destructive',
    icon: <ShieldX className="size-7" />
  },
  failed: {
    wrap: 'border-destructive/50 bg-destructive/12 text-destructive',
    icon: <XCircle className="size-7" />
  }
}

function VerificationGate({ status }: { status: HarnessStatus | null }) {
  const gate = deriveGate(status)
  const s = GATE_STYLES[gate.tone]
  const total = status?.features.features.length ?? 0
  const done = status?.features.features.filter((f) => f.state === 'done').length ?? 0
  return (
    <div className={cn('flex items-center gap-4 rounded-lg border-2 px-5 py-4 transition-colors', s.wrap)}>
      <div className="shrink-0">{s.icon}</div>
      <div className="min-w-0 flex-1">
        <div className="text-lg font-bold tracking-tight">{gate.title}</div>
        <div className="truncate text-sm opacity-80">{gate.detail}</div>
      </div>
      {total > 0 ? (
        <div className="shrink-0 text-right">
          <div className="text-2xl font-bold tabular-nums">
            {done}/{total}
          </div>
          <div className="text-[11px] uppercase tracking-wide opacity-70">verified</div>
        </div>
      ) : null}
    </div>
  )
}

// ---- Feature board ---------------------------------------------------------

const COLUMN_DEFS: { key: FeatureState; label: string }[] = [
  { key: 'pending', label: 'Backlog' },
  { key: 'coding', label: 'Coding' },
  { key: 'verifying', label: 'Verifying' },
  { key: 'done', label: 'Done' }
]

function featureAccent(state: FeatureState): string {
  switch (state) {
    case 'done':
      return 'border-l-[var(--chart-1)]'
    case 'coding':
      return 'border-l-[var(--chart-3)]'
    case 'verifying':
      return 'border-l-[var(--chart-3)]'
    case 'blocked':
      return 'border-l-destructive'
    default:
      return 'border-l-border'
  }
}

function FeatureCard({ feature, isCurrent }: { feature: Feature; isCurrent: boolean }) {
  return (
    <div
      className={cn(
        'rounded-md border border-border border-l-2 bg-background/60 p-2.5',
        featureAccent(feature.state),
        isCurrent && 'ring-1 ring-primary/50'
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <span className="text-[13px] font-medium leading-tight text-foreground">{feature.name}</span>
        {feature.attempts > 0 ? (
          <Badge variant="outline" className="shrink-0 text-[10px]">
            try {feature.attempts}
          </Badge>
        ) : null}
      </div>
      <div className="mt-0.5 font-mono text-[10px] text-muted-foreground">{feature.id}</div>
      {feature.state === 'blocked' && feature.last_error ? (
        <pre className="mt-1.5 max-h-24 overflow-auto rounded bg-destructive/10 p-1.5 font-mono text-[10px] leading-snug text-destructive whitespace-pre-wrap">
          {feature.last_error.slice(-600)}
        </pre>
      ) : null}
    </div>
  )
}

function FeatureBoard({ status }: { status: HarnessStatus | null }) {
  const features = status?.features.features ?? []
  const blocked = features.filter((f) => f.state === 'blocked')
  const columns = blocked.length
    ? [...COLUMN_DEFS, { key: 'blocked' as FeatureState, label: 'Blocked' }]
    : COLUMN_DEFS
  return (
    <div className="grid gap-3" style={{ gridTemplateColumns: `repeat(${columns.length}, minmax(0, 1fr))` }}>
      {columns.map((col) => {
        const items = features.filter((f) => f.state === col.key)
        const isBlockedCol = col.key === 'blocked'
        return (
          <div key={col.key} className="flex flex-col gap-2 rounded-lg border border-border bg-card/40 p-2.5">
            <div className="flex items-center justify-between px-0.5">
              <span
                className={cn(
                  'text-[11px] font-semibold uppercase tracking-wide',
                  isBlockedCol ? 'text-destructive' : 'text-muted-foreground'
                )}
              >
                {col.label}
              </span>
              <span className="text-[11px] tabular-nums text-muted-foreground">{items.length}</span>
            </div>
            <div className="flex flex-col gap-2">
              {items.length === 0 ? (
                <div className="rounded-md border border-dashed border-border/60 py-3 text-center text-[11px] text-muted-foreground/60">
                  empty
                </div>
              ) : (
                items.map((f) => (
                  <FeatureCard key={f.id} feature={f} isCurrent={status?.current_feature === f.id} />
                ))
              )}
            </div>
          </div>
        )
      })}
    </div>
  )
}

// ---- Agent / run status pill ----------------------------------------------

const STATE_LABEL: Record<HarnessState, string> = {
  idle: 'Idle',
  init_verifying: 'Checking env',
  running: 'Running',
  verifying: 'Verifying',
  blocked: 'Blocked',
  done: 'Done',
  failed: 'Failed'
}

function stateDot(state: HarnessState): string {
  switch (state) {
    case 'running':
    case 'init_verifying':
      return 'bg-[var(--chart-3)]'
    case 'verifying':
      return 'bg-[var(--chart-3)] animate-pulse'
    case 'done':
      return 'bg-[var(--chart-1)]'
    case 'blocked':
    case 'failed':
      return 'bg-destructive'
    default:
      return 'bg-muted-foreground/50'
  }
}

// ---- .harness file viewer --------------------------------------------------

const FILE_TABS: { key: keyof HarnessFiles; label: string }[] = [
  { key: 'agents_md', label: 'AGENTS.md' },
  { key: 'feature_list_json', label: 'feature_list.json' },
  { key: 'init_sh', label: 'init.sh' },
  { key: 'verify_sh', label: 'verify.sh' },
  { key: 'handoff_md', label: 'handoff.md' }
]

function HarnessFileViewer({ files }: { files: HarnessFiles | null }) {
  const [active, setActive] = React.useState<keyof HarnessFiles>('agents_md')
  const content = files?.[active]
  return (
    <div className="flex min-h-0 flex-1 flex-col rounded-lg border border-border bg-card/40">
      <div className="flex shrink-0 flex-wrap gap-1 border-b border-border p-1.5">
        {FILE_TABS.map((tab) => (
          <button
            key={tab.key}
            type="button"
            onClick={() => setActive(tab.key)}
            className={cn(
              'rounded px-2 py-1 font-mono text-[11px] transition-colors',
              active === tab.key
                ? 'bg-primary/15 text-foreground'
                : 'text-muted-foreground hover:bg-foreground/8'
            )}
          >
            {tab.label}
          </button>
        ))}
      </div>
      <ScrollArea className="min-h-0 flex-1">
        {content ? (
          <pre className="p-3 font-mono text-[11px] leading-relaxed text-foreground whitespace-pre-wrap">
            {content}
          </pre>
        ) : (
          <div className="p-4 text-[12px] text-muted-foreground/70">
            {files ? 'file not present yet' : 'load a harness to view its .harness/ files'}
          </div>
        )}
      </ScrollArea>
    </div>
  )
}

// ---- Event log -------------------------------------------------------------

const LOG_TONE: Record<LogLine['tone'], string> = {
  info: 'text-muted-foreground',
  good: 'text-[var(--chart-1)]',
  bad: 'text-destructive',
  warn: 'text-[var(--chart-3)]'
}

function EventLog({ log }: { log: LogLine[] }) {
  const endRef = React.useRef<HTMLDivElement | null>(null)
  React.useEffect(() => {
    endRef.current?.scrollIntoView({ block: 'end' })
  }, [log])
  return (
    <div className="flex min-h-0 flex-1 flex-col rounded-lg border border-border bg-card/40">
      <div className="flex shrink-0 items-center gap-1.5 border-b border-border px-3 py-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
        <Terminal className="size-3.5" /> Activity
      </div>
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col gap-0.5 p-3 font-mono text-[11px]">
          {log.length === 0 ? (
            <span className="text-muted-foreground/60">no events yet</span>
          ) : (
            log.map((l) => (
              <div key={l.id} className={LOG_TONE[l.tone]}>
                {l.text}
              </div>
            ))
          )}
          <div ref={endRef} />
        </div>
      </ScrollArea>
    </div>
  )
}

// ---- Page ------------------------------------------------------------------

export default function HarnessEngine(): React.JSX.Element {
  const closeHarnessPage = useAppStore((s) => s.closeHarnessPage)
  const [harnessId, setHarnessId] = React.useState<string | null>(null)
  const [workdir, setWorkdir] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const { status, files, log, refreshStatus, refreshFiles } = useHarnessLive(harnessId)

  // Resume an already-registered run on first mount (e.g. after navigating away).
  React.useEffect(() => {
    let cancelled = false
    void listHarnesses()
      .then((runs) => {
        if (!cancelled && runs.length > 0) {
          setHarnessId(runs[0].id)
          setWorkdir(runs[0].workdir)
        }
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  const onLoad = React.useCallback(async () => {
    if (!workdir.trim()) return
    setBusy(true)
    setError(null)
    try {
      const { harness_id } = await startHarness(workdir.trim())
      setHarnessId(harness_id)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [workdir])

  const onRun = React.useCallback(async () => {
    if (!harnessId) return
    setBusy(true)
    setError(null)
    try {
      await runHarness(harnessId)
      void refreshStatus()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [harnessId, refreshStatus])

  const onStop = React.useCallback(async () => {
    if (!harnessId) return
    setBusy(true)
    try {
      await stopHarness(harnessId)
      setHarnessId(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [harnessId])

  const canRun = !!harnessId && status?.state !== 'running' && status?.state !== 'verifying'

  return (
    <div className="flex h-full min-h-0 flex-col bg-background text-foreground">
      {/* Header */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2.5">
        <Button variant="ghost" size="sm" onClick={closeHarnessPage} className="gap-1.5">
          <ChevronLeft className="size-4" /> Back
        </Button>
        <ShieldCheck className="size-5 text-primary" />
        <span className="text-sm font-semibold tracking-tight">Harness Engine</span>
        {status ? (
          <span className="ml-2 inline-flex items-center gap-1.5 rounded-full bg-card px-2 py-0.5 text-[11px]">
            <span className={cn('size-2 rounded-full', stateDot(status.state))} />
            {STATE_LABEL[status.state]}
          </span>
        ) : null}
        <div className="ml-auto flex items-center gap-2">
          <div className="flex items-center gap-1.5 rounded-md border border-border bg-card px-2">
            <FolderOpen className="size-3.5 text-muted-foreground" />
            <Input
              value={workdir}
              onChange={(e) => setWorkdir(e.target.value)}
              placeholder="/path/to/project (with .harness/)"
              spellCheck={false}
              className="h-8 w-[320px] border-0 bg-transparent px-0 text-[12px] shadow-none focus-visible:ring-0"
              onKeyDown={(e) => {
                if (e.key === 'Enter') void onLoad()
              }}
            />
          </div>
          <Button variant="outline" size="sm" onClick={() => void onLoad()} disabled={busy || !workdir.trim()}>
            Load
          </Button>
          <Button size="sm" onClick={() => void onRun()} disabled={busy || !canRun} className="gap-1.5">
            <Play className="size-3.5" /> Run
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => {
              void refreshStatus()
              void refreshFiles()
            }}
            disabled={!harnessId}
            aria-label="Refresh"
          >
            <RefreshCw className="size-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => void onStop()}
            disabled={!harnessId || busy}
            aria-label="Stop and unload"
          >
            <Square className="size-4" />
          </Button>
        </div>
      </div>

      {error ? (
        <div className="flex shrink-0 items-center gap-2 border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-[12px] text-destructive">
          <AlertTriangle className="size-4 shrink-0" />
          <span className="truncate">{error}</span>
        </div>
      ) : null}

      {/* Body */}
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4">
        <VerificationGate status={status} />

        {status?.workdir ? (
          <div className="flex items-center gap-1.5 font-mono text-[11px] text-muted-foreground">
            <FileText className="size-3.5" />
            {status.workdir}
            {status.current_feature ? (
              <>
                <span className="opacity-50">·</span>
                <span>current: {status.current_feature}</span>
              </>
            ) : null}
          </div>
        ) : null}

        <section>
          <h2 className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
            Feature board
          </h2>
          <FeatureBoard status={status} />
        </section>

        <section className="grid min-h-[280px] grid-cols-2 gap-4">
          <HarnessFileViewer files={files} />
          <EventLog log={log} />
        </section>
      </div>
    </div>
  )
}
