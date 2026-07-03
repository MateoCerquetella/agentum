import React, { useCallback } from 'react'
import { SquareTerminal } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { WorktreeTerminalRow } from './worktree-terminal-rows'

type Props = {
  row: WorktreeTerminalRow
  /** Highlight when this terminal is the active tab in the active worktree. */
  isActive?: boolean
  onActivate: (tabId: string) => void
}

/**
 * Plain-terminal row in a worktree card's inline list, rendered beside the
 * agent rows. Kept intentionally lightweight (icon + title, click to open) —
 * a plain shell has no agent state, send target, or lineage, so it does not
 * reuse the agent row component. Visual chrome (hover/active wash, height,
 * indentation) matches the compact agent row so the two read as one list.
 */
const WorktreeCardTerminalRow = React.memo(function WorktreeCardTerminalRow({
  row,
  isActive = false,
  onActivate
}: Props) {
  const handleActivate = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation()
      onActivate(row.tabId)
    },
    [onActivate, row.tabId]
  )

  return (
    <div
      draggable={false}
      className={cn(
        'group/worktree-terminal-row flex h-6 min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1',
        'text-[11px] leading-none text-muted-foreground worktree-agent-row-hover',
        isActive && 'bg-sidebar-accent'
      )}
      onClick={handleActivate}
      onMouseDown={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
      onDragStart={(e) => e.stopPropagation()}
      role="button"
      tabIndex={-1}
      data-terminal-tab-id={row.tabId}
      title={row.title}
      aria-label={`Open terminal ${row.title}`}
    >
      <SquareTerminal
        className={cn(
          'size-3.5 shrink-0',
          // Dim a terminal with no live PTY (e.g. slept) so live ones stand out,
          // without hiding it — a freshly-created terminal must still show.
          row.hasLivePty ? 'text-muted-foreground/80' : 'text-muted-foreground/45'
        )}
        aria-hidden
      />
      <span
        className={cn(
          'min-w-0 flex-1 truncate',
          row.hasLivePty ? 'text-foreground/85' : 'text-muted-foreground/70'
        )}
      >
        {row.title}
      </span>
    </div>
  )
})

export default WorktreeCardTerminalRow
