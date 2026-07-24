import { LayoutGrid, List } from 'lucide-react'
import type { LinearCustomViewModel, LinearIssue } from '../../../shared/types'
import { type LinearDisplayProperty, type LinearGroupBy, type LinearOrderBy } from './linear-helpers'

export type LinearPresetId = 'assigned' | 'created' | 'all' | 'completed'

type LinearPreset = { id: LinearPresetId; label: string }

export const LINEAR_PRESETS: LinearPreset[] = [
  { id: 'all', label: 'All' },
  { id: 'assigned', label: 'My Issues' },
  { id: 'created', label: 'Created' },
  { id: 'completed', label: 'Completed' }
]

export type LinearViewMode = 'list' | 'board'

export type LinearMode = 'issues' | 'projects' | 'views'

export type LinearProjectTab = 'overview' | 'issues'

export type LinearIssueListRow =
  | { type: 'section'; key: string; label: string; count: number }
  | { type: 'issue'; issue: LinearIssue }

export const LINEAR_MODE_OPTIONS: { id: LinearMode; label: string }[] = [
  { id: 'issues', label: 'Issues' },
  { id: 'projects', label: 'Projects' },
  { id: 'views', label: 'Views' }
]

export const LINEAR_CUSTOM_VIEW_MODEL_OPTIONS: { id: LinearCustomViewModel; label: string }[] = [
  { id: 'issue', label: 'Issues' },
  { id: 'project', label: 'Projects' }
]

export const LINEAR_VIEW_OPTIONS: {
  id: LinearViewMode
  label: string
  Icon: typeof List
}[] = [
  { id: 'list', label: 'List', Icon: List },
  { id: 'board', label: 'Board', Icon: LayoutGrid }
]

export const LINEAR_GROUP_OPTIONS: { id: LinearGroupBy; label: string }[] = [
  { id: 'none', label: 'No grouping' },
  { id: 'status', label: 'Status' },
  { id: 'assignee', label: 'Assignee' },
  { id: 'priority', label: 'Priority' },
  { id: 'team', label: 'Team' }
]

export const LINEAR_ORDER_OPTIONS: { id: LinearOrderBy; label: string }[] = [
  { id: 'priority', label: 'Priority' },
  { id: 'updated', label: 'Updated' },
  { id: 'identifier', label: 'Identifier' }
]

export const LINEAR_DISPLAY_PROPERTIES: { id: LinearDisplayProperty; label: string }[] = [
  { id: 'state', label: 'Status' },
  { id: 'priority', label: 'Priority' },
  { id: 'assignee', label: 'Assignee' },
  { id: 'team', label: 'Team' },
  { id: 'labels', label: 'Labels' },
  { id: 'updated', label: 'Updated' }
]

export const DEFAULT_LINEAR_DISPLAY_PROPERTIES: LinearDisplayProperty[] = [
  'state',
  'priority',
  'assignee',
  'team',
  'labels',
  'updated'
]
