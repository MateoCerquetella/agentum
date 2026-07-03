import { getTaskPresetQuery } from '@/lib/new-workspace'
import { parseTaskQuery } from '@/shared/task-query'
import type { TaskViewPresetId } from '@/shared/types'

export type TaskQueryPreset = {
  id: TaskViewPresetId
  label: string
  query: string
}
export type GitHubTaskKind = 'issues' | 'prs'

const ISSUE_TASK_QUERY_PRESETS: TaskQueryPreset[] = [
  { id: 'issues', label: 'Open', query: getTaskPresetQuery('issues') },
  { id: 'my-issues', label: 'Assigned to me', query: getTaskPresetQuery('my-issues') }
]

const PR_TASK_QUERY_PRESETS: TaskQueryPreset[] = [
  { id: 'prs', label: 'Open', query: getTaskPresetQuery('prs') },
  { id: 'my-prs', label: 'Mine', query: getTaskPresetQuery('my-prs') },
  { id: 'review', label: 'Needs review', query: getTaskPresetQuery('review') }
]

export function getGitHubTaskKindPresets(kind: GitHubTaskKind): TaskQueryPreset[] {
  return kind === 'prs' ? PR_TASK_QUERY_PRESETS : ISSUE_TASK_QUERY_PRESETS
}

export type GitHubModeButton = { id: GitHubTaskKind | 'project'; label: string }

export const GITHUB_MODE_BUTTONS: GitHubModeButton[] = [
  { id: 'issues', label: 'Issues' },
  { id: 'prs', label: 'PRs' },
  { id: 'project', label: 'Projects' }
]

function isPRFocusedTaskView(preset: TaskViewPresetId | null, query: string): boolean {
  if (preset === 'prs' || preset === 'my-prs' || preset === 'review') {
    return true
  }
  const parsed = parseTaskQuery(query)
  return (
    parsed.scope === 'pr' ||
    parsed.state === 'merged' ||
    parsed.draft ||
    parsed.reviewRequested !== null ||
    parsed.reviewedBy !== null
  )
}

export function normalizeGitHubTaskPreset(preset: TaskViewPresetId | null | undefined): TaskViewPresetId {
  // Why: the split Issues/PRs tabs no longer have a mixed "All" view, so
  // legacy saved defaults should land on the first tab instead of mixing rows.
  return !preset || preset === 'all' ? 'issues' : preset
}

export function getGitHubTaskKind(preset: TaskViewPresetId | null, query: string): GitHubTaskKind {
  return isPRFocusedTaskView(preset, query) ? 'prs' : 'issues'
}

export function getDefaultPresetForGitHubTaskKind(kind: GitHubTaskKind): TaskViewPresetId {
  return kind === 'prs' ? 'prs' : 'issues'
}

export function scopeGitHubTaskSearch(query: string, kind: GitHubTaskKind): string {
  const trimmed = query.trim()
  if (!trimmed) {
    return getTaskPresetQuery(getDefaultPresetForGitHubTaskKind(kind))
  }
  if (/\bis:(?:issue|pr|pull-request)\b/i.test(trimmed)) {
    return trimmed
  }
  const parsed = parseTaskQuery(trimmed)
  const inferredKind = parsed.scope === 'pr' ? 'prs' : parsed.scope === 'issue' ? 'issues' : kind
  return `${inferredKind === 'prs' ? 'is:pr' : 'is:issue'} ${trimmed}`
}
