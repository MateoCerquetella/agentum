import React from 'react'
import { Badge } from '@/components/ui/badge'
import { CircleAlert, CircleCheck, CircleDot, Clock, Eye, FlaskConical } from 'lucide-react'
import { cn } from '@/lib/utils'
import { deriveTrackerChip, type TrackerPhaseWire } from '@/lib/tracker-phase'
import { useAppStore } from '@/store'

// Spec 014 F2 (AC 5): the tracker-phase chip — the issue's PIPELINE phase
// (Todo → In Progress → In Review → Ready to Test → Done, written by agentum
// itself), distinct from the issue open/closed badge and the agent-activity
// dot. Sourced from the persisted `trackerPhase` (cold truth) + the live
// `tracker.*` event overlay in the tracker-phase slice; the derivation is the
// pure `deriveTrackerChip`. Renders NOTHING for an unbound worktree (AC 6).

const PHASE_ICONS: Record<TrackerPhaseWire, React.ComponentType<{ className?: string }>> = {
  todo: CircleDot,
  in_progress: Clock,
  in_review: Eye,
  ready_to_test: FlaskConical,
  done: CircleCheck
}

const PHASE_TONES: Record<TrackerPhaseWire, string> = {
  todo: 'border-border bg-muted/30 text-muted-foreground',
  in_progress: 'border-sky-500/25 bg-sky-500/5 text-sky-600 dark:text-sky-300',
  in_review: 'border-purple-500/25 bg-purple-500/5 text-purple-600 dark:text-purple-300',
  ready_to_test: 'border-amber-500/25 bg-amber-500/5 text-amber-600 dark:text-amber-300',
  done: 'border-emerald-500/25 bg-emerald-500/5 text-emerald-600 dark:text-emerald-300'
}

const ATTENTION_TONE = 'border-rose-500/40 bg-rose-500/10 text-rose-600 dark:text-rose-300'

export function TrackerPhaseChip({
  worktreeId,
  persistedPhase
}: {
  worktreeId: string
  persistedPhase?: string | null
}): React.JSX.Element | null {
  const live = useAppStore((s) => s.trackerLiveByWorktreeId[worktreeId])
  const chip = deriveTrackerChip(persistedPhase, live)
  if (!chip) {
    return null
  }
  const Icon = chip.attention ? CircleAlert : chip.phase ? PHASE_ICONS[chip.phase] : CircleAlert
  const tone = chip.attention ? ATTENTION_TONE : chip.phase ? PHASE_TONES[chip.phase] : ATTENTION_TONE
  const label = chip.attention ? `${chip.label} · needs attention` : chip.label
  return (
    <Badge
      variant="outline"
      className={cn(
        'h-4 gap-1 rounded px-1.5 text-[9px] font-medium leading-none [&>svg]:size-2.5',
        tone
      )}
    >
      <Icon />
      <span>{label}</span>
    </Badge>
  )
}
