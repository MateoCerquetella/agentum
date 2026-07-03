import React, { useCallback } from 'react'
import { Globe } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { BrowserWorkspace } from '../../../../shared/types'
import { writeBrowserTabDragData } from './workspace-status'

type Props = {
  workspace: BrowserWorkspace
  /** Highlight when this browser is the active tab in the active worktree. */
  isActive?: boolean
  onActivate: (workspaceId: string) => void
}

/**
 * Browser-tab row in a worktree card's inline list, rendered beside the agent
 * and terminal rows. Click opens/focuses it; DRAG it onto another worktree card
 * to MOVE the browser there — native HTML5 drag carrying the workspace id into
 * the card's `BROWSER_TAB_DRAG_TYPE` drop handler (which calls
 * `moveBrowserTabToWorktree`). Chrome matches `WorktreeCardTerminalRow` so the
 * three row types read as one list.
 */
const WorktreeCardBrowserRow = React.memo(function WorktreeCardBrowserRow({
  workspace,
  isActive = false,
  onActivate
}: Props) {
  const handleActivate = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation()
      onActivate(workspace.id)
    },
    [onActivate, workspace.id]
  )

  const label = workspace.title?.trim() || 'New Tab'

  return (
    <div
      draggable
      // Stop the parent worktree card's dnd-kit reorder from hijacking the press
      // (same trick the terminal row uses); then native HTML5 drag carries the
      // workspace id to a worktree card's drop handler = "move to that worktree".
      onPointerDown={(e) => e.stopPropagation()}
      onMouseDown={(e) => e.stopPropagation()}
      onDragStart={(e) => {
        e.stopPropagation()
        writeBrowserTabDragData(e.dataTransfer, workspace.id)
      }}
      className={cn(
        'group/worktree-browser-row flex h-6 min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1',
        'text-[11px] leading-none text-muted-foreground worktree-agent-row-hover',
        isActive && 'bg-sidebar-accent'
      )}
      onClick={handleActivate}
      role="button"
      tabIndex={-1}
      data-browser-workspace-id={workspace.id}
      title={label}
      aria-label={`Open browser ${label} (drag onto a worktree to move it there)`}
    >
      <Globe className="size-3.5 shrink-0 text-sky-500/80" aria-hidden />
      <span
        className={cn(
          'min-w-0 flex-1 truncate',
          isActive ? 'text-foreground/85' : 'text-foreground/70'
        )}
      >
        {label}
      </span>
    </div>
  )
})

export default WorktreeCardBrowserRow
