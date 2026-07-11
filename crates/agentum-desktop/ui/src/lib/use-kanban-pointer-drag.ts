// Pointer-event drag-and-drop for column kanbans. See kanban-pointer-drag.ts
// for the DOM contract and for why this exists instead of HTML5 DnD (wry's
// native drag handler starves in-page drag events on Linux/Windows).
//
// Modeled on the sidebar workspace kanban's pointer drag, simplified to the
// single-card / column-target case the boards need: pointerdown-capture on the
// board, a movement threshold so plain clicks stay clicks, a cloned card
// preview following the pointer, elementFromPoint column hit-testing, and a
// brief click swallow after a real drag so the drop doesn't also open the
// card's dialog.
import { useCallback, useEffect, useRef, useState } from 'react'
import type React from 'react'

import {
  KANBAN_DRAG_THRESHOLD_PX,
  findKanbanDragCard,
  resolveKanbanDropColumn,
  shouldStartKanbanPointerDrag,
  type KanbanDomElement
} from './kanban-pointer-drag'

type DragState = {
  pointerId: number
  startX: number
  startY: number
  currentX: number
  currentY: number
  cardId: string
  sourceColumnKey: string | null
  card: HTMLElement
  preview: HTMLElement | null
  started: boolean
  frameId: number | null
}

function setDragDocumentStyles(active: boolean): void {
  document.body.style.cursor = active ? 'grabbing' : ''
  document.body.style.userSelect = active ? 'none' : ''
}

// A translucent clone pinned at the card's rect; the original stays in place
// (dimmed by the caller via dragCardId) so the column layout never shifts
// mid-drag. pointer-events:none keeps it invisible to elementFromPoint.
function createDragPreview(card: HTMLElement): HTMLElement {
  const rect = card.getBoundingClientRect()
  const preview = card.cloneNode(true) as HTMLElement
  preview.style.position = 'fixed'
  preview.style.left = `${rect.left}px`
  preview.style.top = `${rect.top}px`
  preview.style.width = `${rect.width}px`
  preview.style.height = `${rect.height}px`
  preview.style.margin = '0'
  preview.style.pointerEvents = 'none'
  preview.style.zIndex = '9999'
  preview.style.opacity = '0.9'
  preview.style.boxShadow = '0 8px 24px rgb(0 0 0 / 0.25)'
  document.body.appendChild(preview)
  return preview
}

type UseKanbanPointerDragParams = {
  boardRef: React.RefObject<HTMLElement | null>
  /** Fired on drop over a droppable column other than the card's own. */
  onDrop: (cardId: string, columnKey: string) => void
}

export function useKanbanPointerDrag({ boardRef, onDrop }: UseKanbanPointerDragParams): {
  /** Card id of an in-flight drag (threshold passed), for dimming the original. */
  dragCardId: string | null
  /** Droppable column currently under the pointer, for the hover highlight. */
  overColumnKey: string | null
  onBoardPointerDownCapture: (event: React.PointerEvent<HTMLElement>) => void
} {
  const dragRef = useRef<DragState | null>(null)
  const suppressClickUntilRef = useRef(0)
  const [dragCardId, setDragCardId] = useState<string | null>(null)
  const [overColumnKey, setOverColumnKey] = useState<string | null>(null)
  // Why: the document-level listeners are mounted once; the drop callback must
  // still see the caller's latest closure when they run.
  const onDropRef = useRef(onDrop)
  onDropRef.current = onDrop

  const columnAt = useCallback(
    (x: number, y: number): string | null => {
      const board = boardRef.current
      if (!board) {
        return null
      }
      const hit = document.elementFromPoint(x, y)
      return resolveKanbanDropColumn(hit as KanbanDomElement | null, board as KanbanDomElement)
    },
    [boardRef]
  )

  const stopDrag = useCallback(
    (commit: boolean) => {
      const state = dragRef.current
      if (!state) {
        return
      }
      dragRef.current = null
      if (state.frameId !== null) {
        window.cancelAnimationFrame(state.frameId)
      }
      state.preview?.remove()
      setDragDocumentStyles(false)
      setDragCardId(null)
      setOverColumnKey(null)

      if (!state.started) {
        return
      }
      suppressClickUntilRef.current = performance.now() + 250
      if (!commit) {
        return
      }
      const target = columnAt(state.currentX, state.currentY)
      if (target && target !== state.sourceColumnKey) {
        onDropRef.current(state.cardId, target)
      }
    },
    [columnAt]
  )

  const flushDragFrame = useCallback(() => {
    const state = dragRef.current
    if (!state) {
      return
    }
    state.frameId = null
    if (!state.started) {
      return
    }
    if (state.preview) {
      const dx = state.currentX - state.startX
      const dy = state.currentY - state.startY
      state.preview.style.transform = `translate3d(${dx}px, ${dy}px, 0)`
    }
    setOverColumnKey(columnAt(state.currentX, state.currentY))
  }, [columnAt])

  const scheduleDragFrame = useCallback(
    (state: DragState) => {
      if (state.frameId === null) {
        state.frameId = window.requestAnimationFrame(flushDragFrame)
      }
    },
    [flushDragFrame]
  )

  useEffect(() => {
    const handlePointerMove = (event: PointerEvent): void => {
      const state = dragRef.current
      if (!state || event.pointerId !== state.pointerId) {
        return
      }
      state.currentX = event.clientX
      state.currentY = event.clientY
      if (!state.started) {
        const distance = Math.hypot(event.clientX - state.startX, event.clientY - state.startY)
        if (distance < KANBAN_DRAG_THRESHOLD_PX) {
          return
        }
        state.started = true
        state.preview = createDragPreview(state.card)
        setDragDocumentStyles(true)
        setDragCardId(state.cardId)
      }
      event.preventDefault()
      scheduleDragFrame(state)
    }

    const handlePointerUp = (event: PointerEvent): void => {
      const state = dragRef.current
      if (!state || event.pointerId !== state.pointerId) {
        return
      }
      state.currentX = event.clientX
      state.currentY = event.clientY
      if (state.started) {
        event.preventDefault()
      }
      stopDrag(true)
    }

    const handlePointerCancel = (event: PointerEvent): void => {
      if (dragRef.current && event.pointerId === dragRef.current.pointerId) {
        stopDrag(false)
      }
    }

    // Swallow the click that follows a completed drag — otherwise the drop
    // also activates the card's open-dialog button.
    const handleClick = (event: MouseEvent): void => {
      if (performance.now() > suppressClickUntilRef.current) {
        return
      }
      event.preventDefault()
      event.stopPropagation()
      event.stopImmediatePropagation()
    }

    const handleBlur = (): void => stopDrag(false)

    document.addEventListener('pointermove', handlePointerMove, true)
    document.addEventListener('pointerup', handlePointerUp, true)
    document.addEventListener('pointercancel', handlePointerCancel, true)
    document.addEventListener('click', handleClick, true)
    window.addEventListener('blur', handleBlur)
    return () => {
      document.removeEventListener('pointermove', handlePointerMove, true)
      document.removeEventListener('pointerup', handlePointerUp, true)
      document.removeEventListener('pointercancel', handlePointerCancel, true)
      document.removeEventListener('click', handleClick, true)
      window.removeEventListener('blur', handleBlur)
      stopDrag(false)
    }
  }, [scheduleDragFrame, stopDrag])

  const onBoardPointerDownCapture = useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      if (dragRef.current || !shouldStartKanbanPointerDrag(event.nativeEvent)) {
        return
      }
      const board = boardRef.current
      if (!board || !(event.target instanceof Element)) {
        return
      }
      const found = findKanbanDragCard(
        event.target as unknown as KanbanDomElement,
        board as unknown as KanbanDomElement
      )
      if (!found) {
        return
      }
      dragRef.current = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        currentX: event.clientX,
        currentY: event.clientY,
        cardId: found.cardId,
        sourceColumnKey: found.sourceColumnKey,
        card: found.card as unknown as HTMLElement,
        preview: null,
        started: false,
        frameId: null
      }
    },
    [boardRef]
  )

  return { dragCardId, overColumnKey, onBoardPointerDownCapture }
}
