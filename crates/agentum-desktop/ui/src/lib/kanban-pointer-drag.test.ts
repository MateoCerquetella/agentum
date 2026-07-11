import { describe, expect, it } from 'vitest'

import {
  KANBAN_CARD_ID_ATTR,
  KANBAN_COLUMN_DROPPABLE_ATTR,
  KANBAN_COLUMN_KEY_ATTR,
  KANBAN_NO_DRAG_ATTR,
  findKanbanDragCard,
  resolveKanbanDropColumn,
  shouldStartKanbanPointerDrag,
  type KanbanDomElement
} from './kanban-pointer-drag'

class FakeElement implements KanbanDomElement {
  parentElement: FakeElement | null = null
  private readonly attributes = new Map<string, string>()

  constructor(
    readonly tagName: string = 'DIV',
    attributes: Record<string, string> = {}
  ) {
    for (const [name, value] of Object.entries(attributes)) {
      this.attributes.set(name, value)
    }
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null
  }

  append(...children: FakeElement[]): this {
    for (const child of children) {
      child.parentElement = this
    }
    return this
  }
}

function makeBoard(): {
  board: FakeElement
  column: FakeElement
  card: FakeElement
  cardBody: FakeElement
} {
  const board = new FakeElement()
  const column = new FakeElement('SECTION', { [KANBAN_COLUMN_KEY_ATTR]: 'todo' })
  const card = new FakeElement('DIV', { [KANBAN_CARD_ID_ATTR]: 'card-1' })
  const cardBody = new FakeElement('BUTTON')
  board.append(column)
  column.append(card)
  card.append(cardBody)
  return { board, column, card, cardBody }
}

describe('shouldStartKanbanPointerDrag', () => {
  const base = { button: 0, pointerType: 'mouse', shiftKey: false, metaKey: false, ctrlKey: false }

  it('accepts a plain primary-button mouse press', () => {
    expect(shouldStartKanbanPointerDrag(base)).toBe(true)
  })

  it('rejects secondary buttons, touch, and modifier presses', () => {
    expect(shouldStartKanbanPointerDrag({ ...base, button: 2 })).toBe(false)
    expect(shouldStartKanbanPointerDrag({ ...base, pointerType: 'touch' })).toBe(false)
    expect(shouldStartKanbanPointerDrag({ ...base, shiftKey: true })).toBe(false)
    expect(shouldStartKanbanPointerDrag({ ...base, metaKey: true })).toBe(false)
    expect(shouldStartKanbanPointerDrag({ ...base, ctrlKey: true })).toBe(false)
  })
})

describe('findKanbanDragCard', () => {
  it('resolves the card and its source column from a press inside the card', () => {
    const { board, card, cardBody } = makeBoard()
    expect(findKanbanDragCard(cardBody, board)).toEqual({
      card,
      cardId: 'card-1',
      sourceColumnKey: 'todo'
    })
  })

  it('returns null when the press lands inside an opted-out cluster', () => {
    const { board, card } = makeBoard()
    const actions = new FakeElement('DIV', { [KANBAN_NO_DRAG_ATTR]: '' })
    const actionButton = new FakeElement('BUTTON')
    card.append(actions)
    actions.append(actionButton)
    expect(findKanbanDragCard(actionButton, board)).toBeNull()
  })

  it('returns null for typing/link targets inside the card', () => {
    const { board, card } = makeBoard()
    const input = new FakeElement('INPUT')
    const link = new FakeElement('A', { href: 'https://example.com' })
    card.append(input, link)
    expect(findKanbanDragCard(input, board)).toBeNull()
    expect(findKanbanDragCard(link, board)).toBeNull()
  })

  it('returns null when the card has no id attribute (inert card)', () => {
    const { board, card, cardBody } = makeBoard()
    const inertColumn = new FakeElement('SECTION', { [KANBAN_COLUMN_KEY_ATTR]: 'done' })
    const inertCard = new FakeElement()
    const inertBody = new FakeElement('BUTTON')
    board.append(inertColumn)
    inertColumn.append(inertCard)
    inertCard.append(inertBody)
    expect(findKanbanDragCard(inertBody, board)).toBeNull()
    // Sanity: the draggable sibling still resolves.
    expect(findKanbanDragCard(cardBody, board)?.card).toBe(card)
  })

  it('returns null when the card is outside the board', () => {
    const { board } = makeBoard()
    const strayCard = new FakeElement('DIV', { [KANBAN_CARD_ID_ATTR]: 'stray' })
    const strayBody = new FakeElement('BUTTON')
    strayCard.append(strayBody)
    expect(findKanbanDragCard(strayBody, board)).toBeNull()
    void board
  })
})

describe('resolveKanbanDropColumn', () => {
  it('resolves the column key walking up from the hit element', () => {
    const { board, cardBody } = makeBoard()
    expect(resolveKanbanDropColumn(cardBody, board)).toBe('todo')
  })

  it('returns null for a column marked non-droppable', () => {
    const { board } = makeBoard()
    const readOnly = new FakeElement('SECTION', {
      [KANBAN_COLUMN_KEY_ATTR]: 'no-value',
      [KANBAN_COLUMN_DROPPABLE_ATTR]: 'false'
    })
    const body = new FakeElement()
    board.append(readOnly)
    readOnly.append(body)
    expect(resolveKanbanDropColumn(body, board)).toBeNull()
  })

  it('returns null outside the board or off any column', () => {
    const { board, column } = makeBoard()
    const elsewhere = new FakeElement('SECTION', { [KANBAN_COLUMN_KEY_ATTR]: 'todo' })
    const elsewhereBody = new FakeElement()
    elsewhere.append(elsewhereBody)
    expect(resolveKanbanDropColumn(elsewhereBody, board)).toBeNull()
    expect(resolveKanbanDropColumn(null, board)).toBeNull()
    expect(resolveKanbanDropColumn(board, board)).toBeNull()
    void column
  })
})
