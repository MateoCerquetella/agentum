// Spec 023 Part A (AC 1): the visible "Gated run starting…" state shown in
// place of the bare WorkspaceAgentLauncher picker while an owned gated run
// boots. Renders inside Terminal.tsx's `absolute inset-0 z-20` overlay and
// self-centres (the WorkspaceAgentLauncher contract). Reflects the run's live
// HarnessState/phase/current feature when the hook has a snapshot; degrades
// to generic copy during the create-beat before the first list read lands.
import React from 'react'
import { Loader2 } from 'lucide-react'

export type GatedRunStartingPanelProps = {
  /** The run's `HarnessState` when known (`running`, `init_verifying`, …). */
  stateLabel: string | null
  /** The SDD phase when roles are on, else null. */
  phaseLabel: string | null
  /** Current feature's display name when the drive loop is on one. */
  featureLabel: string | null
}

export function GatedRunStartingPanel({
  stateLabel,
  phaseLabel,
  featureLabel
}: GatedRunStartingPanelProps): React.JSX.Element {
  const detail = [stateLabel, phaseLabel, featureLabel].filter(Boolean).join(' · ')
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 bg-background px-6 text-center">
      <Loader2 className="size-6 animate-spin text-muted-foreground" aria-hidden />
      <p className="text-sm font-medium text-foreground">Gated run starting…</p>
      <p className="max-w-md text-xs text-muted-foreground">
        The harness engine is spawning an agent into this workspace — it appears here as soon as it
        is ready.
        {detail !== '' ? <span className="mt-1 block text-foreground/70">{detail}</span> : null}
      </p>
    </div>
  )
}
