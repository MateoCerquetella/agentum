import type { LinearIssue } from '@/shared/types'

// Why: Linear encodes priority as an integer (0–4). Map to human-readable
// labels so the table column is scannable without memorising the scale.
export const LINEAR_PRIORITY_LABELS: Record<number, string> = {
  0: 'None',
  1: 'Urgent',
  2: 'High',
  3: 'Medium',
  4: 'Low'
}

export type LinearGroupBy = 'none' | 'status' | 'assignee' | 'priority' | 'team'
export type LinearOrderBy = 'priority' | 'updated' | 'identifier'
export type LinearDisplayProperty = 'state' | 'priority' | 'assignee' | 'team' | 'labels' | 'updated'

export type LinearGroupSection = {
  key: string
  label: string
  issues: LinearIssue[]
}

export function getLinearPriorityLabel(priority: number): string {
  return LINEAR_PRIORITY_LABELS[priority] ?? `P${priority}`
}

export function getLinearPriorityRank(priority: number): number {
  return priority === 0 ? 5 : priority
}

export function compareLinearIssues(a: LinearIssue, b: LinearIssue, orderBy: LinearOrderBy): number {
  if (orderBy === 'updated') {
    return new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime()
  }
  if (orderBy === 'identifier') {
    return a.identifier.localeCompare(b.identifier, undefined, { numeric: true })
  }

  const priorityDelta = getLinearPriorityRank(a.priority) - getLinearPriorityRank(b.priority)
  if (priorityDelta !== 0) {
    return priorityDelta
  }
  return new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime()
}

export function getLinearIssueGroup(
  issue: LinearIssue,
  groupBy: LinearGroupBy
): { key: string; label: string } {
  if (groupBy === 'status') {
    return { key: `status:${issue.state.name}`, label: issue.state.name }
  }
  if (groupBy === 'assignee') {
    return {
      key: `assignee:${issue.assignee?.id ?? 'unassigned'}`,
      label: issue.assignee?.displayName ?? 'Unassigned'
    }
  }
  if (groupBy === 'priority') {
    return {
      key: `priority:${issue.priority}`,
      label: getLinearPriorityLabel(issue.priority)
    }
  }
  if (groupBy === 'team') {
    return { key: `team:${issue.team.id}`, label: issue.team.name }
  }
  return { key: 'all', label: 'Issues' }
}

export function groupLinearIssues(
  issues: LinearIssue[],
  groupBy: LinearGroupBy,
  orderBy: LinearOrderBy
): LinearGroupSection[] {
  const sorted = [...issues].sort((a, b) => compareLinearIssues(a, b, orderBy))
  if (groupBy === 'none') {
    return [{ key: 'all', label: 'Issues', issues: sorted }]
  }

  const sections = new Map<string, LinearGroupSection>()
  for (const issue of sorted) {
    const group = getLinearIssueGroup(issue, groupBy)
    const section = sections.get(group.key)
    if (section) {
      section.issues.push(issue)
    } else {
      sections.set(group.key, { key: group.key, label: group.label, issues: [issue] })
    }
  }
  return [...sections.values()]
}

export function getLinearIssueGridTemplate(visibleProperties: ReadonlySet<LinearDisplayProperty>): string {
  const columns = ['96px', 'minmax(180px,1.4fr)']
  if (visibleProperties.has('state')) {
    columns.push('140px')
  }
  if (visibleProperties.has('priority')) {
    columns.push('92px')
  }
  if (visibleProperties.has('assignee')) {
    columns.push('150px')
  }
  if (visibleProperties.has('team')) {
    columns.push('160px')
  }
  if (visibleProperties.has('updated')) {
    columns.push('100px')
  }
  columns.push('72px')
  return columns.join(' ')
}
