// Generic, provider-agnostic Kanban board for the Tasks view. Renders columns
// and draggable cards; dropping a card on a different column fires `onMove` with
// the target column so the caller can push the new state back to the tracker
// (two-way). Pointer-event drag via useKanbanPointerDrag — HTML5 drag-and-drop
// never fires inside the Tauri webview on Linux/Windows (see
// lib/kanban-pointer-drag.ts).
import { useRef } from 'react'

import { cn } from '@/lib/utils'
import { useKanbanPointerDrag } from '@/lib/use-kanban-pointer-drag'
import type { KanbanColumn, KanbanColumnKey } from './task-kanban'

type Props<T> = {
  columns: readonly KanbanColumn[]
  items: readonly T[]
  /** Stable id for a card (used as the drag payload + React key). */
  idOf: (item: T) => string
  /** Which column an item currently lives in. */
  columnOf: (item: T) => KanbanColumnKey
  /** Card body. The board owns the draggable wrapper; this renders the contents. */
  renderCard: (item: T) => React.ReactNode
  /** Fired when a card is dropped on a different column. */
  onMove: (item: T, target: KanbanColumnKey) => void
  /** Id of a card with an in-flight transition (shows a pending affordance). */
  busyId?: string | null
}

export function TaskKanbanBoard<T>({
  columns,
  items,
  idOf,
  columnOf,
  renderCard,
  onMove,
  busyId
}: Props<T>): React.JSX.Element {
  const boardRef = useRef<HTMLDivElement>(null)
  const { dragCardId, overColumnKey, onBoardPointerDownCapture } = useKanbanPointerDrag({
    boardRef,
    onDrop: (cardId, columnKey) => {
      const item = items.find((it) => idOf(it) === cardId)
      const target = columns.find((col) => col.key === columnKey)?.key
      // Only fire when the column actually changes — avoids a no-op tracker write.
      if (item && target && columnOf(item) !== target) {
        onMove(item, target)
      }
    }
  })

  const byColumn = (key: KanbanColumnKey): T[] => items.filter((it) => columnOf(it) === key)

  return (
    <div
      ref={boardRef}
      onPointerDownCapture={onBoardPointerDownCapture}
      className="flex h-full min-h-0 gap-3 overflow-x-auto p-3"
    >
      {columns.map((col) => {
        const cards = byColumn(col.key)
        const isOver = overColumnKey === col.key
        return (
          <section
            key={col.key}
            data-kanban-column-key={col.key}
            className={cn(
              'flex w-72 flex-none flex-col rounded-lg border bg-card/40 transition-colors',
              isOver ? 'border-primary/60 bg-primary/5' : 'border-border'
            )}
          >
            <div className="flex items-center gap-2 border-b border-border px-3 py-2">
              <span className="text-[12px] font-medium tracking-tight">{col.label}</span>
              <span className="rounded-full bg-foreground/8 px-1.5 py-px font-mono text-[10px] text-muted-foreground">
                {cards.length}
              </span>
            </div>
            <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto p-2">
              {cards.length === 0 ? (
                <div className="rounded-md border border-dashed border-border/60 px-3 py-6 text-center text-[11px] text-muted-foreground">
                  Drop here
                </div>
              ) : (
                cards.map((item) => {
                  const id = idOf(item)
                  const busy = busyId === id
                  return (
                    <div
                      key={id}
                      data-kanban-card-id={busy ? undefined : id}
                      className={cn(
                        'rounded-md border border-border bg-background p-2.5 text-[13px] shadow-sm',
                        busy ? 'cursor-wait opacity-60' : 'cursor-grab active:cursor-grabbing hover:border-foreground/30',
                        dragCardId === id && 'opacity-50'
                      )}
                    >
                      {renderCard(item)}
                    </div>
                  )
                })
              )}
            </div>
          </section>
        )
      })}
    </div>
  )
}
