// Kanban (Board-layout) renderer for a GitHub Projects v2 view. Columns come
// from the view's group-by field (via `boardColumns`); dragging a card to
// another column writes the group-by field back to GitHub through the same
// optimistic `onEditField` path the table cells use, so the card moves
// instantly and rolls back on failure. Native HTML5 drag-and-drop — the same
// approach as components/tasks/TaskKanbanBoard.tsx (no extra dependency).
import React, { useMemo, useState } from 'react'
import { cn } from '@/lib/utils'
import {
  boardColumns,
  type ProjectBoardColumn
} from '../../../../shared/github-project-group-sort'
import type {
  GitHubProjectFieldMutationValue,
  GitHubProjectRow,
  GitHubProjectTable
} from '../../../../shared/github-project-types'
import ProjectBoardCard from './ProjectBoardCard'

// GitHub single-select colors are keywords ("YELLOW"), not hex — mirror the
// Primer dark-mode palette ProjectCell uses so a column swatch matches the
// table view's Status pill.
const SINGLE_SELECT_HEX: Record<string, string> = {
  GRAY: '#8b949e',
  RED: '#f85149',
  ORANGE: '#db6d28',
  YELLOW: '#d29922',
  GREEN: '#3fb950',
  BLUE: '#58a6ff',
  PURPLE: '#bc8cff',
  PINK: '#db61a2'
}

function optionDotColor(color: string | null): string {
  if (!color) {
    return 'var(--muted-foreground)'
  }
  const hex = SINGLE_SELECT_HEX[color.toUpperCase()]
  if (hex) {
    return hex
  }
  if (/^#?[0-9a-fA-F]{6}$/.test(color)) {
    return color.startsWith('#') ? color : `#${color}`
  }
  return 'var(--muted-foreground)'
}

type Props = {
  table: GitHubProjectTable
  onOpenDialog?: (row: GitHubProjectRow) => void
  onEditField?: (
    row: GitHubProjectRow,
    fieldId: string,
    value: GitHubProjectFieldMutationValue | null
  ) => void
  onStartWork?: (row: GitHubProjectRow) => void
  onOpenInBrowser?: (row: GitHubProjectRow) => void
}

export default function ProjectBoardView({
  table,
  onOpenDialog,
  onEditField,
  onStartWork,
  onOpenInBrowser
}: Props): React.JSX.Element {
  const board = useMemo(() => boardColumns(table), [table])
  const [overKey, setOverKey] = useState<string | null>(null)

  // rowId → current column key, and rowId → row, so a drop can suppress a
  // same-column no-op and resolve the dragged row without a linear scan.
  const { columnKeyByRowId, rowById } = useMemo(() => {
    const columnKeyByRowId = new Map<string, string>()
    const rowById = new Map<string, GitHubProjectRow>()
    for (const col of board.columns) {
      for (const row of col.rows) {
        columnKeyByRowId.set(row.id, col.key)
        rowById.set(row.id, row)
      }
    }
    return { columnKeyByRowId, rowById }
  }, [board.columns])

  const field = board.field
  // Cards can be dragged only when the board can persist a move (the group-by
  // field is single-select or iteration). Read-only boards render static cards.
  const canDrag = field?.kind === 'single-select' || field?.kind === 'iteration'

  const handleDrop = (col: ProjectBoardColumn, e: React.DragEvent): void => {
    e.preventDefault()
    setOverKey(null)
    if (!col.droppable || !field || !onEditField) {
      return
    }
    const id = e.dataTransfer.getData('text/plain')
    if (!id || columnKeyByRowId.get(id) === col.key) {
      return
    }
    const row = rowById.get(id)
    if (row) {
      onEditField(row, field.id, col.moveValue)
    }
  }

  return (
    <div className="flex h-full min-h-0 gap-3 overflow-x-auto p-3 scrollbar-sleek">
      {board.columns.map((col) => {
        const isOver = overKey === col.key && col.droppable
        return (
          <section
            key={col.key}
            onDragOver={(e) => {
              if (!col.droppable) {
                return
              }
              e.preventDefault()
              if (overKey !== col.key) {
                setOverKey(col.key)
              }
            }}
            onDragLeave={(e) => {
              // Only clear when leaving the column itself, not a child card.
              if (e.currentTarget === e.target) {
                setOverKey(null)
              }
            }}
            onDrop={(e) => handleDrop(col, e)}
            className={cn(
              'flex w-72 flex-none flex-col rounded-lg border bg-card/40 transition-colors',
              isOver ? 'border-primary/60 bg-primary/5' : 'border-border'
            )}
          >
            <div className="flex items-center gap-2 border-b border-border px-3 py-2">
              <span
                className="size-2.5 flex-none rounded-full"
                style={{ backgroundColor: optionDotColor(col.color) }}
              />
              <span className="truncate text-[12px] font-medium tracking-tight">{col.label}</span>
              <span className="ml-auto rounded-full bg-foreground/8 px-1.5 py-px font-mono text-[10px] text-muted-foreground">
                {col.rows.length}
              </span>
            </div>
            <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto p-2 scrollbar-sleek">
              {col.rows.length === 0 ? (
                <div className="rounded-md border border-dashed border-border/60 px-3 py-6 text-center text-[11px] text-muted-foreground">
                  {col.droppable ? 'Drop here' : 'Empty'}
                </div>
              ) : (
                col.rows.map((row) => (
                  <ProjectBoardCard
                    key={row.id}
                    row={row}
                    draggable={canDrag && row.itemType !== 'REDACTED'}
                    onDragStart={(e) => {
                      e.dataTransfer.setData('text/plain', row.id)
                      e.dataTransfer.effectAllowed = 'move'
                    }}
                    onOpenDialog={onOpenDialog ? () => onOpenDialog(row) : undefined}
                    onOpenInBrowser={onOpenInBrowser ? () => onOpenInBrowser(row) : undefined}
                    onStartWork={onStartWork ? () => onStartWork(row) : undefined}
                  />
                ))
              )}
            </div>
          </section>
        )
      })}
    </div>
  )
}
