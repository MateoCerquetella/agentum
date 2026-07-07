// Why: cover the bug fixes from the recent review — particularly the NaN
// sort produced when two rows reference unknown single-select option IDs or
// unknown iteration IDs, and the empty-group ordering invariant.
import { describe, expect, it } from 'vitest'
import type {
  GitHubProjectField,
  GitHubProjectRow,
  GitHubProjectSort,
  GitHubProjectTable,
  GitHubProjectView
} from '../../../../shared/github-project-types'
import { sortRows, groupRows, boardColumns } from '../../../../shared/github-project-group-sort'

const singleSelectField: GitHubProjectField = {
  kind: 'single-select',
  id: 'F_status',
  name: 'Status',
  dataType: 'SINGLE_SELECT',
  options: [
    { id: 'opt_a', name: 'Todo', color: 'GRAY' },
    { id: 'opt_b', name: 'In Progress', color: 'YELLOW' }
  ]
}

const iterationField: GitHubProjectField = {
  kind: 'iteration',
  id: 'F_iter',
  name: 'Iteration',
  dataType: 'ITERATION',
  iterations: [
    { id: 'iter_1', title: 'Sprint 1', startDate: '2026-01-01', duration: 14, completed: false },
    { id: 'iter_2', title: 'Sprint 2', startDate: '2026-01-15', duration: 14, completed: false }
  ]
}

function makeRow(
  id: string,
  position: number,
  values: GitHubProjectRow['fieldValuesByFieldId']
): GitHubProjectRow {
  return {
    id,
    itemType: 'ISSUE',
    content: {
      number: 1,
      title: id,
      body: null,
      url: null,
      state: 'open',
      stateReason: null,
      isDraft: null,
      repository: 'acme/repo',
      assignees: [],
      labels: [],
      parentIssue: null,
      issueType: null
    },
    fieldValuesByFieldId: values,
    updatedAt: '2026-01-01T00:00:00Z',
    position
  }
}

function makeView(field: GitHubProjectField, sort?: GitHubProjectSort): GitHubProjectView {
  return {
    id: 'V_1',
    number: 1,
    name: 'Default',
    layout: 'TABLE_LAYOUT',
    filter: '',
    fields: [field],
    groupByFields: [],
    sortByFields: sort ? [sort] : []
  }
}

function makeTable(view: GitHubProjectView, rows: GitHubProjectRow[]): GitHubProjectTable {
  return {
    project: {
      id: 'P',
      owner: 'acme',
      ownerType: 'organization',
      number: 1,
      title: 'P',
      url: ''
    },
    selectedView: view,
    rows,
    totalCount: rows.length,
    parentFieldDropped: false
  }
}

describe('sortRows', () => {
  it('orders rows by single-select option order', () => {
    const view = makeView(singleSelectField, {
      direction: 'ASC',
      field: singleSelectField
    })
    const rows = [
      makeRow('r2', 1, {
        F_status: {
          kind: 'single-select',
          fieldId: 'F_status',
          optionId: 'opt_b',
          name: 'In Progress',
          color: 'YELLOW'
        }
      }),
      makeRow('r1', 0, {
        F_status: {
          kind: 'single-select',
          fieldId: 'F_status',
          optionId: 'opt_a',
          name: 'Todo',
          color: 'GRAY'
        }
      })
    ]
    const sorted = sortRows(makeTable(view, rows), rows)
    expect(sorted.map((r) => r.id)).toEqual(['r1', 'r2'])
  })

  it('does not produce NaN when both rows reference unknown single-select options', () => {
    // Why: this was the bug — `Infinity - Infinity = NaN` made sort()'s
    // behavior implementation-defined and skipped the row.position
    // tie-break. Two orphaned rows must still fall through to position.
    const view = makeView(singleSelectField, {
      direction: 'ASC',
      field: singleSelectField
    })
    const rows = [
      makeRow('rB', 5, {
        F_status: {
          kind: 'single-select',
          fieldId: 'F_status',
          optionId: 'orphan_2',
          name: 'Gone',
          color: 'GRAY'
        }
      }),
      makeRow('rA', 1, {
        F_status: {
          kind: 'single-select',
          fieldId: 'F_status',
          optionId: 'orphan_1',
          name: 'Gone',
          color: 'GRAY'
        }
      })
    ]
    const sorted = sortRows(makeTable(view, rows), rows)
    // After tie-break by position, rA (position=1) precedes rB (position=5).
    expect(sorted.map((r) => r.id)).toEqual(['rA', 'rB'])
  })

  it('does not produce NaN when both rows reference unknown iteration ids', () => {
    const view = makeView(iterationField, {
      direction: 'ASC',
      field: iterationField
    })
    const rows = [
      makeRow('rB', 5, {
        F_iter: {
          kind: 'iteration',
          fieldId: 'F_iter',
          iterationId: 'gone_b',
          title: 'Gone B',
          startDate: '2025-01-01',
          duration: 14
        }
      }),
      makeRow('rA', 1, {
        F_iter: {
          kind: 'iteration',
          fieldId: 'F_iter',
          iterationId: 'gone_a',
          title: 'Gone A',
          startDate: '2025-01-01',
          duration: 14
        }
      })
    ]
    const sorted = sortRows(makeTable(view, rows), rows)
    expect(sorted.map((r) => r.id)).toEqual(['rA', 'rB'])
  })

  it('places rows missing the sort field after rows that have it', () => {
    const view = makeView(singleSelectField, {
      direction: 'ASC',
      field: singleSelectField
    })
    const rows = [
      makeRow('rEmpty', 0, {}),
      makeRow('rHas', 1, {
        F_status: {
          kind: 'single-select',
          fieldId: 'F_status',
          optionId: 'opt_a',
          name: 'Todo',
          color: 'GRAY'
        }
      })
    ]
    const sorted = sortRows(makeTable(view, rows), rows)
    expect(sorted.map((r) => r.id)).toEqual(['rHas', 'rEmpty'])
  })

  it('keeps sort fallback finite when row positions are absent', () => {
    const view = makeView(singleSelectField)
    const rows = [
      { ...makeRow('rA', 0, {}), position: undefined as unknown as number },
      { ...makeRow('rB', 0, {}), position: undefined as unknown as number }
    ]

    const sorted = sortRows(makeTable(view, rows), rows)

    expect(sorted.map((r) => r.id)).toEqual(['rA', 'rB'])
  })
})

describe('groupRows', () => {
  it('places the empty group last', () => {
    const view = {
      ...makeView(singleSelectField),
      groupByFields: [singleSelectField]
    }
    const rows = [
      makeRow('rNone', 0, {}),
      makeRow('rA', 1, {
        F_status: {
          kind: 'single-select',
          fieldId: 'F_status',
          optionId: 'opt_a',
          name: 'Todo',
          color: 'GRAY'
        }
      })
    ]
    const groups = groupRows(makeTable(view, rows), rows)
    expect(groups.map((g) => g.key)).toEqual(['opt_a', '__empty__'])
  })
})

const statusValue = (optionId: string, name: string) =>
  ({
    F_status: {
      kind: 'single-select' as const,
      fieldId: 'F_status',
      optionId,
      name,
      color: 'GRAY'
    }
  })

function boardView(field: GitHubProjectField): GitHubProjectView {
  return { ...makeView(field), layout: 'BOARD_LAYOUT', groupByFields: [field] }
}

describe('boardColumns', () => {
  it('emits a column per single-select option in order, plus a trailing "No <field>" column', () => {
    const rows = [
      makeRow('rB', 1, statusValue('opt_b', 'In Progress')),
      makeRow('rNone', 2, {}),
      makeRow('rA', 0, statusValue('opt_a', 'Todo'))
    ]
    const { field, columns } = boardColumns(makeTable(boardView(singleSelectField), rows))

    expect(field?.id).toBe('F_status')
    // Option order preserved; empty column last.
    expect(columns.map((c) => c.key)).toEqual(['opt_a', 'opt_b', '__empty__'])
    expect(columns.map((c) => c.label)).toEqual(['Todo', 'In Progress', 'No Status'])
    // Empty option columns still appear (as drop targets) even with no rows —
    // here every option has exactly one row.
    expect(columns.map((c) => c.rows.map((r) => r.id))).toEqual([['rA'], ['rB'], ['rNone']])
  })

  it('makes option columns droppable with a single-select moveValue and the empty column a clear (null)', () => {
    const { columns } = boardColumns(makeTable(boardView(singleSelectField), []))
    const todo = columns.find((c) => c.key === 'opt_a')!
    const none = columns.find((c) => c.key === '__empty__')!

    expect(todo.droppable).toBe(true)
    expect(todo.moveValue).toEqual({ kind: 'single-select', optionId: 'opt_a' })
    expect(none.droppable).toBe(true)
    expect(none.moveValue).toBeNull()
    // Every option renders a column even when the board is empty.
    expect(columns.map((c) => c.key)).toEqual(['opt_a', 'opt_b', '__empty__'])
  })

  it('never drops a row whose option no longer exists — it becomes a read-only column', () => {
    const rows = [
      makeRow('rGhost', 0, statusValue('opt_deleted', 'Archived')),
      makeRow('rA', 1, statusValue('opt_a', 'Todo'))
    ]
    const { columns } = boardColumns(makeTable(boardView(singleSelectField), rows))

    const ghost = columns.find((c) => c.key === 'opt_deleted')
    expect(ghost).toBeDefined()
    expect(ghost!.droppable).toBe(false)
    expect(ghost!.rows.map((r) => r.id)).toEqual(['rGhost'])
    // Orphan columns come after the known options and the empty column.
    expect(columns.map((c) => c.key)).toEqual(['opt_a', 'opt_b', '__empty__', 'opt_deleted'])
  })

  it('builds iteration columns with an iteration moveValue', () => {
    const rows = [
      makeRow('rI', 0, {
        F_iter: {
          kind: 'iteration',
          fieldId: 'F_iter',
          iterationId: 'iter_1',
          title: 'Sprint 1',
          startDate: '2026-01-01',
          duration: 14
        }
      })
    ]
    const { columns } = boardColumns(makeTable(boardView(iterationField), rows))
    expect(columns.map((c) => c.key)).toEqual(['iter_1', 'iter_2', '__empty__'])
    expect(columns.find((c) => c.key === 'iter_1')!.moveValue).toEqual({
      kind: 'iteration',
      iterationId: 'iter_1'
    })
  })

  it('falls back to a single read-only "All" column when the view has no group-by', () => {
    const rows = [makeRow('r1', 0, {}), makeRow('r2', 1, {})]
    const { field, columns } = boardColumns(makeTable(makeView(singleSelectField), rows))
    expect(field).toBeNull()
    expect(columns).toHaveLength(1)
    expect(columns[0].key).toBe('all')
    expect(columns[0].droppable).toBe(false)
    expect(columns[0].rows.map((r) => r.id)).toEqual(['r1', 'r2'])
  })
})
