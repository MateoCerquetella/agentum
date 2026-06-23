// Generic, provider-agnostic Kanban board for the Tasks view. Renders columns
// and draggable cards; dropping a card on a different column fires `onMove` with
// the target column so the caller can push the new state back to the tracker
// (two-way). Uses native HTML5 drag-and-drop — no extra dependency.
import { useState } from 'react'

import { cn } from '@/lib/utils'
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
  // Which column is currently a drop target (for the hover highlight).
  const [overColumn, setOverColumn] = useState<KanbanColumnKey | null>(null)

  const byColumn = (key: KanbanColumnKey): T[] => items.filter((it) => columnOf(it) === key)

  const handleDrop = (target: KanbanColumnKey, e: React.DragEvent): void => {
    e.preventDefault()
    setOverColumn(null)
    const id = e.dataTransfer.getData('text/plain')
    if (!id) return
    const item = items.find((it) => idOf(it) === id)
    // Only fire when the column actually changes — avoids a no-op tracker write.
    if (item && columnOf(item) !== target) onMove(item, target)
  }

  return (
    <div className="flex h-full min-h-0 gap-3 overflow-x-auto p-3">
      {columns.map((col) => {
        const cards = byColumn(col.key)
        const isOver = overColumn === col.key
        return (
          <section
            key={col.key}
            onDragOver={(e) => {
              e.preventDefault()
              if (overColumn !== col.key) setOverColumn(col.key)
            }}
            onDragLeave={(e) => {
              // Only clear when leaving the column itself, not a child card.
              if (e.currentTarget === e.target) setOverColumn(null)
            }}
            onDrop={(e) => handleDrop(col.key, e)}
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
                      draggable={!busy}
                      onDragStart={(e) => {
                        e.dataTransfer.setData('text/plain', id)
                        e.dataTransfer.effectAllowed = 'move'
                      }}
                      className={cn(
                        'rounded-md border border-border bg-background p-2.5 text-[13px] shadow-sm',
                        busy ? 'cursor-wait opacity-60' : 'cursor-grab active:cursor-grabbing hover:border-foreground/30'
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
