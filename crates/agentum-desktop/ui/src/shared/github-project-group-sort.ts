// Why: grouping and sorting of Project rows is deterministic shared logic
// driven by `selectedView` — it must not depend on fetch ordering. Keeping it
// in a pure shared module lets desktop and mobile render Project views the same.
import type {
  GitHubProjectField,
  GitHubProjectFieldMutationValue,
  GitHubProjectRow,
  GitHubProjectSort,
  GitHubProjectTable
} from './github-project-types'

export type ProjectGroup = {
  /** Stable key used for React reconciliation. */
  key: string
  /** Human-readable label used in the group header. */
  label: string
  /** Iteration metadata for headers that render a date range + Current pill. */
  iteration: {
    startDate: string
    duration: number
    completed: boolean
  } | null
  rows: GitHubProjectRow[]
}

const EMPTY_GROUP_KEY = '__empty__'

// Why: use a finite sentinel instead of Infinity so subtractions in the sort
// comparator stay finite. `Infinity - Infinity` is NaN, which makes
// Array.sort's behavior implementation-defined and skips later tie-breaks.
const UNKNOWN_INDEX_SENTINEL = Number.MAX_SAFE_INTEGER

function getFieldValueForGrouping(
  row: GitHubProjectRow,
  field: GitHubProjectField
): { key: string; label: string; orderHint: number; iteration: ProjectGroup['iteration'] } {
  const value = row.fieldValuesByFieldId[field.id]
  if (!value) {
    return {
      key: EMPTY_GROUP_KEY,
      label: labelForEmpty(field),
      orderHint: UNKNOWN_INDEX_SENTINEL,
      iteration: null
    }
  }
  if (field.kind === 'iteration' && value.kind === 'iteration') {
    const idx = field.iterations.findIndex((it) => it.id === value.iterationId)
    const meta = field.iterations.find((it) => it.id === value.iterationId)
    return {
      key: value.iterationId,
      label: value.title || meta?.title || 'Iteration',
      orderHint: idx === -1 ? UNKNOWN_INDEX_SENTINEL - 1 : idx,
      iteration: meta
        ? { startDate: meta.startDate, duration: meta.duration, completed: meta.completed }
        : null
    }
  }
  if (field.kind === 'single-select' && value.kind === 'single-select') {
    const idx = field.options.findIndex((o) => o.id === value.optionId)
    return {
      key: value.optionId,
      label: value.name,
      orderHint: idx === -1 ? UNKNOWN_INDEX_SENTINEL - 1 : idx,
      iteration: null
    }
  }
  const label = deriveStringValue(value)
  return { key: `raw:${label}`, label, orderHint: 0, iteration: null }
}

function labelForEmpty(field: GitHubProjectField): string {
  return `No ${field.name}`
}

function deriveStringValue(value: GitHubProjectRow['fieldValuesByFieldId'][string]): string {
  switch (value.kind) {
    case 'text':
      return value.text
    case 'number':
      return String(value.number)
    case 'date':
      return value.date
    case 'single-select':
      return value.name
    case 'iteration':
      return value.title
    case 'labels':
      return value.labels.map((l) => l.name).join(', ')
    case 'users':
      return value.users.map((u) => u.login).join(', ')
  }
}

export function groupRows(
  table: GitHubProjectTable,
  rowsInOrder: GitHubProjectRow[]
): ProjectGroup[] {
  return groupRowsByField(table.selectedView.groupByFields[0] ?? null, rowsInOrder)
}

// Why: Table grouping and Board columns bucket rows identically but are driven
// by different view fields (`groupByFields` vs `verticalGroupByFields`), so the
// bucketing takes the field explicitly instead of reading it off the view.
function groupRowsByField(
  groupField: GitHubProjectField | null,
  rowsInOrder: GitHubProjectRow[]
): ProjectGroup[] {
  if (!groupField) {
    return [{ key: 'all', label: '', iteration: null, rows: rowsInOrder }]
  }
  const buckets = new Map<
    string,
    {
      label: string
      orderHint: number
      iteration: ProjectGroup['iteration']
      rows: GitHubProjectRow[]
    }
  >()
  for (const row of rowsInOrder) {
    const { key, label, orderHint, iteration } = getFieldValueForGrouping(row, groupField)
    let bucket = buckets.get(key)
    if (!bucket) {
      bucket = { label, orderHint, iteration, rows: [] }
      buckets.set(key, bucket)
    }
    bucket.rows.push(row)
  }
  const entries = Array.from(buckets.entries())
  // Ordering rules per design doc §Grouping.
  entries.sort((a, b) => {
    if (a[0] === EMPTY_GROUP_KEY) {
      return 1
    }
    if (b[0] === EMPTY_GROUP_KEY) {
      return -1
    }
    if (groupField.kind === 'iteration' || groupField.kind === 'single-select') {
      return a[1].orderHint - b[1].orderHint
    }
    return a[1].label.localeCompare(b[1].label)
  })
  return entries.map(([key, v]) => ({
    key,
    label: v.label,
    iteration: v.iteration,
    rows: v.rows
  }))
}

// ─── Board (Kanban) layout ─────────────────────────────────────────────

export type ProjectBoardColumn = {
  /** Stable key: single-select option id / iteration id / EMPTY_GROUP_KEY / 'all'. */
  key: string
  label: string
  /** Single-select option color (a GitHub color name, e.g. 'YELLOW') for a
   *  header swatch; null for columns without one. */
  color: string | null
  rows: GitHubProjectRow[]
  /** True when a card may be dropped here, i.e. the drop maps to a concrete
   *  field write. False for read-only columns (non-editable group-by, or an
   *  orphaned bucket whose option/iteration no longer exists). */
  droppable: boolean
  /** The field mutation applied on drop. `null` clears the field (the
   *  "No <field>" column). Only meaningful when `droppable`. */
  moveValue: GitHubProjectFieldMutationValue | null
}

export type ProjectBoard = {
  /** The group-by field driving the columns, or null when the view has none. */
  field: GitHubProjectField | null
  columns: ProjectBoardColumn[]
}

// Turn a Board-layout view into ordered Kanban columns. Reuses the exact
// bucketing that `groupRows` applies (so a card's column matches its Table
// group header), but — unlike `groupRows` — enumerates the group-by field's
// full option/iteration set so empty columns exist as valid drop targets, and
// appends a trailing "No <field>" column so unset items are visible and the
// field can be cleared by dragging there.
export function boardColumns(table: GitHubProjectTable): ProjectBoard {
  // Why: GitHub exposes a board's column field as `verticalGroupByFields`;
  // `groupByFields` on a Board view is the optional swimlane grouping and is
  // empty for a typical board — reading only it collapsed every board into a
  // single read-only "All" column. Fall back to `groupByFields` for cached
  // payloads that predate `verticalGroupByFields`.
  const field =
    table.selectedView.verticalGroupByFields?.[0] ?? table.selectedView.groupByFields[0] ?? null
  const sorted = sortRows(table, table.rows)
  if (!field) {
    return {
      field: null,
      columns: [
        { key: 'all', label: 'All', color: null, rows: sorted, droppable: false, moveValue: null }
      ]
    }
  }

  // Reuse the Table grouping's bucketing (and its computed labels) so a card's
  // column matches what a group header would show for the same field. Buckets
  // are keyed by option id / iteration id / EMPTY_GROUP_KEY.
  const groups = groupRowsByField(field, sorted)
  const byKey = new Map(groups.map((g) => [g.key, g]))
  const rowsFor = (key: string): GitHubProjectRow[] => byKey.get(key)?.rows ?? []
  const consumed = new Set<string>([EMPTY_GROUP_KEY])
  const emptyColumn: ProjectBoardColumn = {
    key: EMPTY_GROUP_KEY,
    label: labelForEmpty(field),
    color: null,
    rows: rowsFor(EMPTY_GROUP_KEY),
    droppable: true,
    moveValue: null
  }
  // Buckets whose key isn't one of the field's current options/iterations (a
  // deleted option still referenced by a row) — surfaced read-only so those
  // rows are never silently dropped from the board.
  const orphanColumns = (): ProjectBoardColumn[] =>
    groups
      .filter((g) => !consumed.has(g.key))
      .map((g) => ({
        key: g.key,
        label: g.label,
        color: null,
        rows: g.rows,
        droppable: false,
        moveValue: null
      }))

  if (field.kind === 'single-select') {
    const columns = field.options.map<ProjectBoardColumn>((o) => {
      consumed.add(o.id)
      return {
        key: o.id,
        label: o.name,
        color: o.color,
        rows: rowsFor(o.id),
        droppable: true,
        moveValue: { kind: 'single-select', optionId: o.id }
      }
    })
    return { field, columns: [...columns, emptyColumn, ...orphanColumns()] }
  }

  if (field.kind === 'iteration') {
    const columns = field.iterations.map<ProjectBoardColumn>((it) => {
      consumed.add(it.id)
      return {
        key: it.id,
        label: it.title,
        color: null,
        rows: rowsFor(it.id),
        droppable: true,
        moveValue: { kind: 'iteration', iterationId: it.id }
      }
    })
    return { field, columns: [...columns, emptyColumn, ...orphanColumns()] }
  }

  // Any other group-by kind (assignees, labels, text, …): we have no
  // single-write mutation to move a card, so render the populated buckets
  // read-only in their grouped order.
  const columns = groups.map<ProjectBoardColumn>((g) => ({
    key: g.key,
    label: g.label || labelForEmpty(field),
    color: null,
    rows: g.rows,
    droppable: false,
    moveValue: null
  }))
  return { field, columns }
}

function compareSort(a: GitHubProjectRow, b: GitHubProjectRow, sort: GitHubProjectSort): number {
  const field = sort.field
  const aValue = a.fieldValuesByFieldId[field.id]
  const bValue = b.fieldValuesByFieldId[field.id]
  if (!aValue && !bValue) {
    return 0
  }
  if (!aValue) {
    return 1
  }
  if (!bValue) {
    return -1
  }

  let cmp = 0
  if (
    field.kind === 'single-select' &&
    aValue.kind === 'single-select' &&
    bValue.kind === 'single-select'
  ) {
    const aIdx = field.options.findIndex((o) => o.id === aValue.optionId)
    const bIdx = field.options.findIndex((o) => o.id === bValue.optionId)
    cmp =
      (aIdx === -1 ? UNKNOWN_INDEX_SENTINEL : aIdx) - (bIdx === -1 ? UNKNOWN_INDEX_SENTINEL : bIdx)
  } else if (
    field.kind === 'iteration' &&
    aValue.kind === 'iteration' &&
    bValue.kind === 'iteration'
  ) {
    const aIdx = field.iterations.findIndex((it) => it.id === aValue.iterationId)
    const bIdx = field.iterations.findIndex((it) => it.id === bValue.iterationId)
    cmp =
      (aIdx === -1 ? UNKNOWN_INDEX_SENTINEL : aIdx) - (bIdx === -1 ? UNKNOWN_INDEX_SENTINEL : bIdx)
  } else if (aValue.kind === 'number' && bValue.kind === 'number') {
    cmp = aValue.number - bValue.number
  } else if (aValue.kind === 'date' && bValue.kind === 'date') {
    cmp = aValue.date.localeCompare(bValue.date)
  } else if (aValue.kind === 'text' && bValue.kind === 'text') {
    cmp = aValue.text.localeCompare(bValue.text)
  } else if (aValue.kind === 'users' && bValue.kind === 'users') {
    const aLogin = aValue.users[0]?.login ?? ''
    const bLogin = bValue.users[0]?.login ?? ''
    if (!aLogin && !bLogin) {
      cmp = 0
    } else if (!aLogin) {
      cmp = 1
    } else if (!bLogin) {
      cmp = -1
    } else {
      cmp = aLogin.localeCompare(bLogin)
    }
  } else if (aValue.kind === 'labels' && bValue.kind === 'labels') {
    const aName = aValue.labels[0]?.name ?? ''
    const bName = bValue.labels[0]?.name ?? ''
    if (!aName && !bName) {
      cmp = 0
    } else if (!aName) {
      cmp = 1
    } else if (!bName) {
      cmp = -1
    } else {
      cmp = aName.localeCompare(bName)
    }
  } else {
    // Why: unknown sort-field kind — ignore this sort field and fall through
    // to tie-breaks (and eventually row.position).
    return 0
  }
  return sort.direction === 'DESC' ? -cmp : cmp
}

export function sortRows(table: GitHubProjectTable, rows: GitHubProjectRow[]): GitHubProjectRow[] {
  const sorts = table.selectedView.sortByFields
  const out = [...rows]
  out.sort((a, b) => {
    for (const sort of sorts) {
      const cmp = compareSort(a, b, sort)
      if (cmp !== 0) {
        return cmp
      }
    }
    return (a.position ?? UNKNOWN_INDEX_SENTINEL) - (b.position ?? UNKNOWN_INDEX_SENTINEL)
  })
  return out
}

export function isIterationCurrent(iteration: { startDate: string; duration: number }): boolean {
  const start = new Date(`${iteration.startDate}T00:00:00Z`).getTime()
  if (Number.isNaN(start)) {
    return false
  }
  const end = start + iteration.duration * 86_400_000
  const now = Date.now()
  return now >= start && now < end
}
