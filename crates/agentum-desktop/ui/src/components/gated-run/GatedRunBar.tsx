// Spec 023 Part B (AC 7): the workspace's gated-run strip — the run's only
// desktop surface (there is no run panel): live state/phase + current feature
// + the linked tracker issue chip with a two-tap "Unlink issue". Mounted ONCE
// at Terminal.tsx's root beside HarnessSpecBanner (same load-bearing
// `relative z-30`: it must paint above the z-20 launcher overlay). Renders
// null unless a run owns this worktree (matched by workdir) and isn't
// finished — a done/failed run has no transitions left to unlink.
import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Github, Loader2, Unlink } from 'lucide-react'
import { toast } from 'sonner'
import { useAppStore } from '@/store'
import { unlinkHarnessIssue } from '@/runtime/harness-client'
import { linkedIssueLabel, runLinkedIssue } from '@/lib/harness-run'
import { useWorktreeHarnessRun } from '@/hooks/useWorktreeHarnessRun'

export type GatedRunBarViewProps = {
  /** The run's `HarnessState` (`running`, `verifying`, …). */
  state: string
  /** The SDD phase when roles are on (`authoring`…), else null. */
  phase: string | null
  /** Current feature's display name, when the drive loop is on one. */
  currentFeature: string | null
  /** Executing backlog position (`n/N`), null outside an identified feature. */
  featureProgress: string | null
  /** Short linked-issue label (`#42`), null once unlinked/never stamped. */
  issueLabel: string | null
  busy: boolean
  /** Two-tap confirm armed — unlink is UI-irreversible (re-link is a
   *  follow-up spec), so a stray click must not detach the issue. */
  arming: boolean
  onUnlink: () => void
}

/** Pure presentational strip (renderToStaticMarkup-testable, the
 *  HarnessSpecBanner pattern). */
export function GatedRunBarView({
  state,
  phase,
  currentFeature,
  featureProgress,
  issueLabel,
  busy,
  arming,
  onUnlink
}: GatedRunBarViewProps): React.JSX.Element {
  const detail = [state, phase, currentFeature, featureProgress].filter(Boolean).join(' · ')
  return (
    <div className="relative z-30 flex shrink-0 items-center gap-3 border-b border-border bg-card px-3 py-1.5">
      {state === 'blocked' ? (
        <span className="size-3.5 rounded-full bg-amber-500/80" aria-hidden />
      ) : (
        <Loader2 className="size-3.5 animate-spin text-muted-foreground" aria-hidden />
      )}
      <p className="min-w-0 flex-1 truncate text-xs text-foreground">
        <span className="font-medium">Gated run</span>{' '}
        <span className="text-muted-foreground">{detail}</span>
      </p>
      {issueLabel !== null ? (
        <span className="inline-flex shrink-0 items-center gap-1.5">
          <span className="inline-flex items-center gap-1 rounded-full border border-border bg-background px-2 py-0.5 text-[11px] text-muted-foreground">
            <Github className="size-3" aria-hidden />
            {issueLabel}
          </span>
          <button
            type="button"
            onClick={onUnlink}
            disabled={busy}
            className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
              arming
                ? 'border-destructive/60 bg-destructive/10 text-destructive'
                : 'border-border bg-background text-muted-foreground hover:bg-accent hover:text-foreground'
            }`}
          >
            <Unlink className="size-3" aria-hidden />
            {arming ? 'Confirm unlink' : 'Unlink issue'}
          </button>
        </span>
      ) : null}
    </div>
  )
}

/**
 * Store host: resolves the active worktree's path, tracks the engine run
 * owning it, and owns the two-tap unlink flow. The chip clears WITHOUT a
 * reload (AC 7): the engine's `log` event re-reads the status through the
 * hook, and a deterministic `refresh()` rides alongside it.
 */
export default function GatedRunBar({
  worktreeId
}: {
  worktreeId: string
}): React.JSX.Element | null {
  const workdir = useAppStore((s) => {
    const worktree = Object.values(s.worktreesByRepo ?? {})
      .flat()
      .find((w) => w.id === worktreeId)
    return worktree?.path
  })
  const { run, refresh } = useWorktreeHarnessRun(workdir)
  const [busy, setBusy] = useState(false)
  const [arming, setArming] = useState(false)
  const armTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(
    () => () => {
      if (armTimer.current) clearTimeout(armTimer.current)
    },
    []
  )
  // Switching runs disarms any pending confirm.
  useEffect(() => {
    setArming(false)
  }, [run?.id])

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

  if (!run || run.state === 'done' || run.state === 'failed') {
    return null
  }
  const issue = runLinkedIssue(run)
  const currentFeature =
    run.features.features.find((f) => f.id === run.current_feature)?.name ?? null
  const currentFeatureIndex = run.features.features.findIndex((f) => f.id === run.current_feature)
  const featureProgress =
    run.phase === 'executing' && currentFeatureIndex >= 0
      ? `${currentFeatureIndex + 1}/${run.features.features.length}`
      : null
  return (
    <GatedRunBarView
      state={run.state}
      phase={run.phase ?? null}
      currentFeature={currentFeature}
      featureProgress={featureProgress}
      issueLabel={issue !== null ? linkedIssueLabel(issue) : null}
      busy={busy}
      arming={arming}
      onUnlink={handleUnlink}
    />
  )
}
