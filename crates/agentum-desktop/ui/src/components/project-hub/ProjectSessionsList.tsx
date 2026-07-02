// Sessions tab of the Project Hub (ADE redesign): every workspace (worktree)
// of the project as a flat list — status dot, branch, path — with a one-click
// jump into its terminal workbench. This is the same data the sidebar shows,
// re-presented at hub width so a project's sessions can be scanned in one place.
import React, { useMemo } from 'react'
import { ArrowRight, SquareTerminal } from 'lucide-react'

import { useWorktreesForRepo } from '@/store/selectors'
import { useWorktreeActivityStatus } from '@/components/sidebar/use-worktree-activity-status'
import StatusIndicator from '@/components/sidebar/StatusIndicator'
import { activateAndRevealWorktree } from '@/lib/worktree-activation'
import { getWorktreeStatusLabel } from '@/lib/worktree-status'
import type { Worktree } from '@/shared/types'

function shortPath(path: string): string {
  const segments = path.split('/').filter(Boolean)
  return segments.slice(-2).join('/')
}

function SessionRow({ worktree }: { worktree: Worktree }): React.JSX.Element {
  const status = useWorktreeActivityStatus(worktree.id)
  return (
    <button
      type="button"
      onClick={() => void activateAndRevealWorktree(worktree.id)}
      className="group flex w-full items-center gap-3 rounded-lg border border-border bg-card px-4 py-3 text-left transition-colors hover:border-foreground/25 hover:bg-accent/40"
    >
      <StatusIndicator status={status} aria-hidden="true" />
      <span className="sr-only">{getWorktreeStatusLabel(status)}</span>
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-semibold leading-tight">
          {worktree.displayName}
        </div>
        <div className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground">
          {worktree.branch ? `${worktree.branch} · ` : ''}
          {shortPath(worktree.path)}
        </div>
      </div>
      <span className="inline-flex flex-none items-center gap-1 text-[12px] text-muted-foreground transition-colors group-hover:text-foreground">
        Open terminal <ArrowRight className="size-3.5" />
      </span>
    </button>
  )
}

export function ProjectSessionsList({ repoId }: { repoId: string }): React.JSX.Element {
  const worktrees = useWorktreesForRepo(repoId)
  const visible = useMemo(() => worktrees.filter((w) => !w.isArchived), [worktrees])

  if (visible.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
        <SquareTerminal className="size-8 text-muted-foreground/50" />
        <div className="text-sm font-medium">No workspaces yet</div>
        <div className="max-w-[360px] text-[12.5px] text-muted-foreground">
          Create a workspace for this project from the sidebar to start an agent session here.
        </div>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto px-6 py-5">
      <div className="mx-auto flex max-w-[760px] flex-col gap-2.5">
        {visible.map((worktree) => (
          <SessionRow key={worktree.id} worktree={worktree} />
        ))}
      </div>
    </div>
  )
}
