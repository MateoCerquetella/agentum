// Persistent mission-control strip for the gated run owning a workspace.
// It has two jobs that must travel together: render the live SDD/task ledger,
// and follow every engine-owned session into a server-pinned terminal tab.
// The latter fixes the role/QA visibility hole where only feature sessions were
// attachable and attached tabs lacked the agent identity needed by SddBarGate.
import React, { useCallback, useEffect, useRef, useState } from 'react'
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Circle,
  Github,
  Loader2,
  PauseCircle,
  RotateCcw,
  Unlink
} from 'lucide-react'
import { toast } from 'sonner'
import { useAppStore } from '@/store'
import { isTuiAgent } from '@/shared/tui-agent-config'
import { cn } from '@/lib/utils'
import {
  runHarness,
  unlinkHarnessIssue,
  type HarnessFeature,
  type HarnessStatus
} from '@/runtime/harness-client'
import {
  currentHarnessFeature,
  deriveGatedRunStages,
  gatedRunBlocker,
  gatedRunHeadline,
  gatedRunPhaseLabel,
  gatedRunSessionTitle,
  linkedIssueLabel,
  runLinkedIssue
} from '@/lib/harness-run'
import { useWorktreeHarnessRun } from '@/hooks/useWorktreeHarnessRun'

export type GatedRunBarViewProps = {
  run: HarnessStatus
  issueLabel: string | null
  busy: boolean
  arming: boolean
  expanded: boolean
  restarting: boolean
  onToggleExpanded: () => void
  onUnlink: () => void
  onRetry: () => void
  onSelectWorker?: (sessionId: string, taskId: string) => void
}

const FEATURE_LABELS: Record<HarnessFeature['state'], string> = {
  pending: 'Queued',
  coding: 'Coding',
  verifying: 'Unit gate',
  ready_to_test: 'Browser QA',
  awaiting_confirm: 'Needs approval',
  done: 'Done',
  blocked: 'Blocked'
}

function RunStateIcon({ run }: { run: HarnessStatus }): React.JSX.Element {
  if (run.state === 'blocked' || run.state === 'failed') {
    return <AlertTriangle className="size-4 text-amber-500" aria-hidden />
  }
  if (run.state === 'awaiting_confirmation') {
    return <PauseCircle className="size-4 text-sky-500" aria-hidden />
  }
  if (run.state === 'done') {
    return <CheckCircle2 className="size-4 text-emerald-500" aria-hidden />
  }
  return <Loader2 className="size-4 animate-spin text-sky-500" aria-hidden />
}

function FeatureStateIcon({ feature }: { feature: HarnessFeature }): React.JSX.Element {
  if (feature.state === 'done') {
    return <Check className="size-3 text-emerald-500" aria-hidden />
  }
  if (feature.state === 'blocked') {
    return <AlertTriangle className="size-3 text-amber-500" aria-hidden />
  }
  if (feature.state === 'awaiting_confirm') {
    return <PauseCircle className="size-3 text-sky-500" aria-hidden />
  }
  if (feature.state !== 'pending') {
    return <Loader2 className="size-3 animate-spin text-sky-500" aria-hidden />
  }
  return <Circle className="size-3 text-muted-foreground/60" aria-hidden />
}

/** Pure presentational surface; effects and store ownership stay in the host. */
export function GatedRunBarView({
  run,
  issueLabel,
  busy,
  arming,
  expanded,
  restarting,
  onToggleExpanded,
  onUnlink,
  onRetry,
  onSelectWorker
}: GatedRunBarViewProps): React.JSX.Element {
  const stages = deriveGatedRunStages(run)
  const currentFeature = currentHarnessFeature(run)
  const blocker = gatedRunBlocker(run)
  const doneCount = run.features.features.filter((feature) => feature.state === 'done').length
  const phase = run.phase === 'blocked' ? run.blocked_phase : run.phase
  const phaseAttempts = run.phase_attempts ?? 0
  const terminal = run.state === 'done' || run.state === 'failed'

  return (
    <section
      className={cn(
        'relative z-30 shrink-0 border-b bg-card',
        run.state === 'blocked' || run.state === 'failed'
          ? 'border-amber-500/35'
          : 'border-border'
      )}
      aria-label="Gated run progress"
    >
      <div className="flex min-h-9 items-center gap-2.5 px-3 py-1.5">
        <RunStateIcon run={run} />
        <div className="min-w-0 flex-1">
          <p className="truncate text-xs text-foreground">
            <span className="font-semibold">Gated run</span>
            <span className="mx-1.5 text-muted-foreground/50">/</span>
            <span className="text-muted-foreground">{gatedRunHeadline(run)}</span>
          </p>
        </div>
        {phaseAttempts > 0 ? (
          <span className="hidden flex-none font-mono text-[10px] text-muted-foreground sm:inline">
            attempt {phaseAttempts}/{run.features.max_retries}
          </span>
        ) : null}
        {issueLabel !== null ? (
          <span className="inline-flex shrink-0 items-center gap-1.5">
            <span className="inline-flex items-center gap-1 rounded-full border border-border bg-background px-2 py-0.5 text-[11px] text-muted-foreground">
              <Github className="size-3" aria-hidden />
              {issueLabel}
            </span>
            {!terminal ? (
              <button
                type="button"
                onClick={onUnlink}
                disabled={busy}
                className={cn(
                  'inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40',
                  arming
                    ? 'border-destructive/60 bg-destructive/10 text-destructive'
                    : 'border-border bg-background text-muted-foreground hover:bg-accent hover:text-foreground'
                )}
              >
                <Unlink className="size-3" aria-hidden />
                {arming ? 'Confirm unlink' : 'Unlink issue'}
              </button>
            ) : null}
          </span>
        ) : null}
        {run.state === 'failed' || run.state === 'blocked' ? (
          <button
            type="button"
            onClick={onRetry}
            disabled={restarting}
            className="inline-flex flex-none items-center gap-1 rounded-full border border-border bg-background px-2 py-0.5 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
          >
            {restarting ? (
              <Loader2 className="size-3 animate-spin" aria-hidden />
            ) : (
              <RotateCcw className="size-3" aria-hidden />
            )}
            {restarting ? 'Retrying…' : 'Retry run'}
          </button>
        ) : null}
        <button
          type="button"
          onClick={onToggleExpanded}
          className="inline-flex flex-none items-center gap-1 rounded-md px-1.5 py-1 text-[10.5px] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          aria-expanded={expanded}
        >
          {expanded ? <ChevronUp className="size-3.5" /> : <ChevronDown className="size-3.5" />}
          {expanded ? 'Hide progress' : 'Show progress'}
        </button>
      </div>

      {expanded ? (
        <div className="border-t border-border/60 bg-background/65 px-3 py-2.5">
          {stages.length > 0 ? (
            <div className="mb-2.5 grid grid-cols-5 gap-1" aria-label="SDD stages">
              {stages.map((stage, index) => (
                <div key={stage.id} className="relative flex min-w-0 items-center gap-1.5">
                  {index < stages.length - 1 ? (
                    <span
                      className={cn(
                        'absolute left-2 top-1/2 z-0 h-px w-[calc(100%+0.25rem)] -translate-y-1/2',
                        stage.status === 'complete' ? 'bg-emerald-500/45' : 'bg-border'
                      )}
                      aria-hidden
                    />
                  ) : null}
                  <span
                    className={cn(
                      'relative z-10 flex size-4 flex-none items-center justify-center rounded-full border bg-background',
                      stage.status === 'complete' && 'border-emerald-500/60 text-emerald-500',
                      stage.status === 'active' && 'border-sky-500 bg-sky-500/10 text-sky-500',
                      stage.status === 'blocked' && 'border-amber-500 bg-amber-500/10 text-amber-500',
                      stage.status === 'paused' && 'border-sky-500 bg-sky-500/10 text-sky-500',
                      stage.status === 'upcoming' && 'border-border text-muted-foreground/40'
                    )}
                  >
                    {stage.status === 'complete' ? (
                      <Check className="size-2.5" />
                    ) : stage.status === 'blocked' ? (
                      <AlertTriangle className="size-2.5" />
                    ) : stage.status === 'active' ? (
                      <span className="size-1.5 animate-pulse rounded-full bg-sky-500" />
                    ) : (
                      <span className="size-1.5 rounded-full bg-current" />
                    )}
                  </span>
                  <span
                    className={cn(
                      'relative z-10 truncate bg-background px-0.5 text-[10px]',
                      stage.status === 'active' || stage.status === 'blocked'
                        ? 'font-semibold text-foreground'
                        : 'text-muted-foreground'
                    )}
                  >
                    {stage.label}
                  </span>
                </div>
              ))}
            </div>
          ) : null}

          <div className="flex items-center gap-2">
            <span className="flex-none font-mono text-[9.5px] font-semibold uppercase tracking-[0.14em] text-muted-foreground">
              Tasks
            </span>
            <span className="text-[10px] text-muted-foreground">
              {doneCount}/{run.features.features.length} complete
            </span>
            {phase ? (
              <span className="ml-auto truncate text-[10px] text-muted-foreground">
                {gatedRunPhaseLabel(phase)}
                {run.current_agent_tool ? ` · ${run.current_agent_tool}` : ''}
              </span>
            ) : null}
          </div>

          {run.features.features.length > 0 ? (
            <div className="mt-1.5 flex gap-1.5 overflow-x-auto pb-0.5">
              {run.features.features.map((feature, index) => {
                const active = feature.id === currentFeature?.id
                return (
                  <div
                    key={feature.id}
                    title={feature.last_error || feature.description || feature.name}
                    className={cn(
                      'flex min-w-[150px] max-w-[240px] flex-1 items-start gap-2 rounded-md border px-2 py-1.5',
                      feature.state === 'blocked'
                        ? 'border-amber-500/40 bg-amber-500/8'
                        : active
                          ? 'border-sky-500/35 bg-sky-500/8'
                          : 'border-border/70 bg-card'
                    )}
                  >
                    <span className="mt-0.5 flex size-4 flex-none items-center justify-center rounded bg-secondary font-mono text-[9px] text-muted-foreground">
                      {index + 1}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[10.5px] font-medium text-foreground">
                        {feature.name}
                      </span>
                      <span className="mt-0.5 flex items-center gap-1 text-[9.5px] text-muted-foreground">
                        <FeatureStateIcon feature={feature} />
                        {FEATURE_LABELS[feature.state]}
                        {feature.attempts > 0 ? ` · ${feature.attempts} retries` : ''}
                      </span>
                    </span>
                  </div>
                )
              })}
            </div>
          ) : (
            <p className="mt-1.5 rounded-md border border-dashed border-border px-2.5 py-2 text-[10.5px] text-muted-foreground">
              Tasks appear here after the spec and architecture gates produce the build plan.
            </p>
          )}

          {run.execution_mode === 'orchestrated' && (run.active_workers?.length ?? 0) > 0 ? (
            <div className="mt-2" aria-label="Active workers">
              <div className="flex items-center gap-2 text-[9.5px] text-muted-foreground">
                <span className="font-mono font-semibold uppercase tracking-[0.14em]">Workers</span>
                <span>{run.active_workers?.length}/{run.max_concurrency ?? 4} active</span>
                {run.coordinator_session ? (
                  <span className="ml-auto font-mono">coordinator {run.coordinator_session.slice(0, 8)}</span>
                ) : null}
              </div>
              <div className="mt-1.5 flex gap-1.5 overflow-x-auto pb-0.5">
                {run.active_workers?.map((worker) => (
                  <button
                    key={worker.task_id}
                    type="button"
                    disabled={!worker.session_id}
                    onClick={() => worker.session_id && onSelectWorker?.(worker.session_id, worker.task_id)}
                    className={cn(
                      'min-w-[180px] rounded-md border px-2 py-1.5 text-left transition-colors disabled:cursor-default',
                      worker.conflict
                        ? 'border-amber-500/40 bg-amber-500/8'
                        : 'border-sky-500/30 bg-sky-500/5 hover:bg-sky-500/10'
                    )}
                    title={worker.conflict ?? `Open worker terminal for ${worker.task_id}`}
                  >
                    <span className="flex items-center gap-1.5 text-[10.5px] font-medium text-foreground">
                      <Loader2 className="size-3 animate-spin text-sky-500" aria-hidden />
                      <span className="truncate">{worker.task_id}</span>
                      <span className="ml-auto rounded border border-border px-1 py-px font-mono text-[8.5px] text-muted-foreground">
                        {worker.enforcement}
                      </span>
                    </span>
                    <span className="mt-1 flex items-center gap-1 text-[9.5px] text-muted-foreground">
                      {worker.state.replaceAll('_', ' ')}
                      {worker.patch_state ? ` · patch ${worker.patch_state}` : ''}
                      {worker.context_remaining != null ? ` · ${worker.context_remaining}% ctx` : ''}
                    </span>
                    {worker.conflict ? (
                      <span className="mt-1 block truncate text-[9.5px] text-amber-600">{worker.conflict}</span>
                    ) : null}
                  </button>
                ))}
              </div>
            </div>
          ) : null}

          {blocker ? (
            <div className="mt-2 flex items-start gap-2 rounded-md border border-amber-500/35 bg-amber-500/8 px-2.5 py-2">
              <AlertTriangle className="mt-0.5 size-3.5 flex-none text-amber-500" aria-hidden />
              <p className="max-h-20 min-w-0 overflow-y-auto whitespace-pre-wrap text-[10.5px] leading-4 text-foreground/85">
                {blocker}
              </p>
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  )
}

export default function GatedRunBar({
  worktreeId
}: {
  worktreeId: string
}): React.JSX.Element | null {
  const pending = useAppStore(
    (state) => state.gatedRunStartingByWorktreeId[worktreeId] !== undefined
  )
  const workdir = useAppStore((state) => {
    const worktree = Object.values(state.worktreesByRepo ?? {})
      .flat()
      .find((entry) => entry.id === worktreeId)
    return worktree?.path
  })
  const allowLegacyLocalPathFallback = useAppStore((state) => {
    const worktree = Object.values(state.worktreesByRepo ?? {})
      .flat()
      .find((entry) => entry.id === worktreeId)
    if (!worktree) return false
    const repo = state.repos?.find((entry) => entry.id === worktree.repoId)
    return repo ? !repo.connectionId : false
  })
  const { run, refresh } = useWorktreeHarnessRun(
    workdir,
    worktreeId,
    allowLegacyLocalPathFallback
  )
  const [busy, setBusy] = useState(false)
  const [restarting, setRestarting] = useState(false)
  const [arming, setArming] = useState(false)
  const [expanded, setExpanded] = useState(true)
  const armTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const attachedSessionIds = useRef(new Set<string>())

  useEffect(
    () => () => {
      if (armTimer.current) clearTimeout(armTimer.current)
    },
    []
  )
  useEffect(() => {
    setArming(false)
  }, [run?.id])

  // Follow every current session (role → feature → QA → reviewer), not merely
  // the first feature session observed while the empty-state overlay is up.
  // New stages activate only when the user is already following this run (or
  // the workspace is still empty); manual work in another tab is not stolen.
  useEffect(() => {
    const sessionId = run?.current_session
    if (!run || !sessionId) return
    const state = useAppStore.getState()
    const tabs = state.tabsByWorktree[worktreeId] ?? []
    const existing = tabs.find((tab) => tab.serverSessionId === sessionId)
    const active = tabs.find((tab) => tab.id === state.activeTabId)
    const followingRun =
      pending ||
      tabs.length === 0 ||
      Boolean(active?.serverSessionId && attachedSessionIds.current.has(active.serverSessionId))
    attachedSessionIds.current.add(sessionId)

    if (existing) {
      if (followingRun) state.setActiveTab(existing.id)
    } else {
      const rawAgent = run.current_agent_tool ?? run.features.agent_tool
      const tab = state.createTab(worktreeId, undefined, undefined, {
        activate: followingRun,
        recordInteraction: false,
        persistTmux: true,
        serverSessionId: sessionId,
        ...(isTuiAgent(rawAgent) ? { launchAgent: rawAgent } : {})
      })
      state.setTabCustomTitle(tab.id, gatedRunSessionTitle(run))
    }
    state.clearGatedRunStarting(worktreeId)
  }, [pending, run, worktreeId])

  const handleUnlink = useCallback((): void => {
    if (!run || busy) return
    if (!arming) {
      setArming(true)
      armTimer.current = setTimeout(() => setArming(false), 3000)
      return
    }
    if (armTimer.current) {
      clearTimeout(armTimer.current)
      armTimer.current = null
    }
    setArming(false)
    setBusy(true)
    unlinkHarnessIssue(run.id)
      .then(() => {
        toast.success('Issue unlinked — status updates stopped.')
        refresh()
      })
      .catch((error: unknown) => {
        toast.error(error instanceof Error ? error.message : String(error))
      })
      .finally(() => setBusy(false))
  }, [run, busy, arming, refresh])

  const handleRetry = useCallback((): void => {
    if (!run || restarting) return
    setRestarting(true)
    runHarness(run.id)
      .then(() => {
        toast.success('Gated run restarted.')
        refresh()
      })
      .catch((error: unknown) => {
        toast.error(error instanceof Error ? error.message : String(error))
      })
      .finally(() => setRestarting(false))
  }, [refresh, restarting, run])

  const handleSelectWorker = useCallback((sessionId: string, taskId: string): void => {
    const state = useAppStore.getState()
    const tabs = state.tabsByWorktree[worktreeId] ?? []
    const existing = tabs.find((tab) => tab.serverSessionId === sessionId)
    if (existing) {
      state.setActiveTab(existing.id)
      return
    }
    const rawAgent = run?.current_agent_tool ?? run?.features.agent_tool
    const tab = state.createTab(worktreeId, undefined, undefined, {
      activate: true,
      recordInteraction: false,
      persistTmux: true,
      serverSessionId: sessionId,
      ...(rawAgent && isTuiAgent(rawAgent) ? { launchAgent: rawAgent } : {})
    })
    state.setTabCustomTitle(tab.id, `Worker · ${taskId}`)
  }, [run, worktreeId])

  if (!run) {
    return pending ? (
      <section
        className="relative z-30 shrink-0 border-b border-border bg-card"
        aria-label="Gated run progress"
      >
        <div className="flex min-h-9 items-center gap-2.5 px-3 py-1.5">
          <Loader2 className="size-4 animate-spin text-sky-500" aria-hidden />
          <p className="truncate text-xs text-foreground">
            <span className="font-semibold">Gated run</span>
            <span className="mx-1.5 text-muted-foreground/50">/</span>
            <span className="text-muted-foreground">Starting SDD Autopilot…</span>
          </p>
        </div>
      </section>
    ) : null
  }
  const issue = runLinkedIssue(run)
  return (
    <GatedRunBarView
      run={run}
      issueLabel={issue !== null ? linkedIssueLabel(issue) : null}
      busy={busy}
      arming={arming}
      expanded={expanded}
      restarting={restarting}
      onToggleExpanded={() => setExpanded((value) => !value)}
      onUnlink={handleUnlink}
      onRetry={handleRetry}
      onSelectWorker={handleSelectWorker}
    />
  )
}
