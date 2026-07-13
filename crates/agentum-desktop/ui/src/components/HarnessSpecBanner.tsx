// Spec 015: dismissible "Start Harness run" offer for a just-created workspace
// whose workdir already carries a harness spec. Mounted ONCE at Terminal.tsx's
// root strip (above both the launcher overlay and the split surfaces — the
// #313 lesson: never mount a new surface only in the legacy fallback). Renders
// null unless the offer slice has an entry for exactly this worktree.
import React, { useCallback, useState } from 'react'
import { X } from 'lucide-react'
import { Button } from './ui/button'
import { useAppStore } from '@/store'

export type HarnessSpecBannerViewProps = {
  harnessDir: string
  busy: boolean
  onAccept: () => void
  onDismiss: () => void
}

/** Pure presentational strip. `relative z-30` is load-bearing: the launcher
 *  empty-state overlay is `absolute inset-0 z-20` over the whole root box, so
 *  an unpositioned strip would paint UNDER it; `z-30` keeps the banner above
 *  while the launcher's self-centered card stays clear and interactive.
 *  `shrink-0` + flex-child placement means sibling `flex-1` surfaces shrink
 *  instead of being occluded (AC 2: non-blocking). */
export function HarnessSpecBannerView({
  harnessDir,
  busy,
  onAccept,
  onDismiss
}: HarnessSpecBannerViewProps): React.JSX.Element {
  return (
    <div className="relative z-30 flex shrink-0 items-center gap-3 border-b border-border bg-card px-3 py-2">
      <p className="min-w-0 flex-1 text-sm text-foreground">
        <span className="font-medium">Harness spec found</span>{' '}
        <span className="text-muted-foreground">
          ({harnessDir}/feature_list.json) — start a gated run on this workspace?
        </span>
      </p>
      {/* Both actions disabled while busy: the double-accept guard (belt and
          braces alongside the server's claim_driver). */}
      <Button size="sm" onClick={onAccept} disabled={busy}>
        Start Harness run
      </Button>
      <button
        type="button"
        aria-label="Dismiss harness offer"
        onClick={onDismiss}
        disabled={busy}
        className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
      >
        <X className="size-3.5" />
      </button>
    </div>
  )
}

/**
 * Store host: selects the offer for exactly this worktree (switching
 * worktrees hides/re-shows correctly; a workspace that wasn't just created
 * never has an entry). Dismiss clears the slice entry and performs no other
 * writes (AC 4); only the creation-time runner ever re-sets it (D2).
 */
export default function HarnessSpecBanner({
  worktreeId
}: {
  worktreeId: string
}): React.JSX.Element | null {
  const offer = useAppStore((s) => s.harnessOfferByWorktreeId[worktreeId])
  const clearOffer = useAppStore((s) => s.clearWorkspaceHarnessOffer)
  const [busy, setBusy] = useState(false)

  const handleAccept = useCallback(() => {
    // f3 wires acceptHarnessOffer here; until then accept is a busy no-op.
    void setBusy
  }, [])

  const handleDismiss = useCallback(() => {
    clearOffer(worktreeId)
  }, [clearOffer, worktreeId])

  if (!offer) {
    return null
  }
  return (
    <HarnessSpecBannerView
      harnessDir={offer.harnessDir}
      busy={busy}
      onAccept={handleAccept}
      onDismiss={handleDismiss}
    />
  )
}
