// Pointer-drag DOM contract + pure helpers shared by the kanban boards
// (Tasks GitHub board, GitHub Projects board, TaskPage's Linear board).
//
// Why pointer events instead of HTML5 drag-and-drop: the Tauri shell keeps the
// native drag-drop handler enabled (`dragDropEnabled` defaults to true) so OS
// file drops keep working — dragging a screenshot onto an agent terminal goes
// through `WindowEvent::DragDrop` in crates/agentum-desktop/src/lib.rs. On
// Linux and Windows wry consumes the native drag loop to feed that handler, so
// in-page HTML5 drag events (dragstart/dragover/drop) never fire inside the
// webview — the kanbans rendered grab cursors but silently refused to drag.
// Pointer events bypass the native drag machinery entirely; the sidebar
// workspace kanban (use-workspace-kanban-card-pointer-drag.ts) and project
// reordering (project-header-drag.ts) already work this way.
//
// The DOM contract, set as literal data-* attributes at the render sites:
// - data-kanban-card-id                    on a draggable card (omit → inert)
// - data-kanban-column-key                 on a column drop target
// - data-kanban-column-droppable="false"   on a column that only renders
// - data-kanban-no-drag                    on interactive clusters inside a
//   card (hover action buttons) whose clicks must never be eaten by a jittery
//   drag — pointerdowns there never start one.

export const KANBAN_CARD_ID_ATTR = 'data-kanban-card-id'
export const KANBAN_COLUMN_KEY_ATTR = 'data-kanban-column-key'
export const KANBAN_COLUMN_DROPPABLE_ATTR = 'data-kanban-column-droppable'
export const KANBAN_NO_DRAG_ATTR = 'data-kanban-no-drag'

export const KANBAN_DRAG_THRESHOLD_PX = 5

/** The slice of Element the helpers touch — lets tests use plain fakes. */
export type KanbanDomElement = {
  parentElement: KanbanDomElement | null
  getAttribute(name: string): string | null
  tagName: string
}

export function shouldStartKanbanPointerDrag(
  event: Pick<PointerEvent, 'button' | 'pointerType' | 'shiftKey' | 'metaKey' | 'ctrlKey'>
): boolean {
  if (event.button !== 0 || event.pointerType === 'touch') {
    return false
  }
  // Why: modifier gestures are reserved for selection/context-menu intent —
  // same rule as the workspace kanban.
  return !event.shiftKey && !event.metaKey && !event.ctrlKey
}

// Elements a drag must never start from even inside a card: typing/link
// targets, where swallowing the click (or the text-selection drag) is hostile.
// Plain buttons are deliberately draggable — most cards are wall-to-wall
// buttons, so refusing them would leave nothing to grab; the post-drag click
// suppression keeps their activation intact.
const NON_DRAGGABLE_TAGS = new Set(['INPUT', 'TEXTAREA', 'SELECT', 'OPTION'])

function refusesDrag(el: KanbanDomElement): boolean {
  if (el.getAttribute(KANBAN_NO_DRAG_ATTR) !== null) {
    return true
  }
  if (NON_DRAGGABLE_TAGS.has(el.tagName)) {
    return true
  }
  if (el.tagName === 'A' && el.getAttribute('href') !== null) {
    return true
  }
  return el.getAttribute('contenteditable') === 'true'
}

export type KanbanDragCard<E extends KanbanDomElement> = {
  card: E
  cardId: string
  /** Column the card currently lives in — a drop back onto it is a no-op. */
  sourceColumnKey: string | null
}

/**
 * Resolve the draggable card for a pointerdown at `target`, walking up to
 * `board`. Null when the press landed on an opted-out element before reaching
 * a card, when no card id is present, or when the chain never reaches `board`.
 */
export function findKanbanDragCard<E extends KanbanDomElement>(
  target: E | null,
  board: E
): KanbanDragCard<E> | null {
  let card: E | null = null
  let cardId: string | null = null
  let sourceColumnKey: string | null = null
  let inBoard = false
  for (let el: E | null = target; el; el = el.parentElement as E | null) {
    if (!card) {
      if (refusesDrag(el)) {
        return null
      }
      const id = el.getAttribute(KANBAN_CARD_ID_ATTR)
      if (id) {
        card = el
        cardId = id
      }
    }
    if (card && sourceColumnKey === null) {
      sourceColumnKey = el.getAttribute(KANBAN_COLUMN_KEY_ATTR)
    }
    if (el === board) {
      inBoard = true
      break
    }
  }
  if (!card || !cardId || !inBoard) {
    return null
  }
  return { card, cardId, sourceColumnKey }
}

/**
 * Resolve the droppable column under the pointer from the hit-test element
 * (`document.elementFromPoint` — the drag preview is pointer-events:none so it
 * never shadows the hit). Null outside `board`, on no column, or on a column
 * marked non-droppable.
 */
export function resolveKanbanDropColumn(
  hit: KanbanDomElement | null,
  board: KanbanDomElement
): string | null {
  let key: string | null = null
  let inBoard = false
  for (let el: KanbanDomElement | null = hit; el; el = el.parentElement) {
    if (key === null) {
      const candidate = el.getAttribute(KANBAN_COLUMN_KEY_ATTR)
      if (candidate !== null) {
        if (el.getAttribute(KANBAN_COLUMN_DROPPABLE_ATTR) === 'false') {
          return null
        }
        key = candidate
      }
    }
    if (el === board) {
      inBoard = true
      break
    }
  }
  return key !== null && inBoard ? key : null
}
