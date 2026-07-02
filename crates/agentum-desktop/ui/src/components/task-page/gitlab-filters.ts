export type GitLabTaskFilter = 'opened' | 'merged' | 'closed' | 'all'
export type GitLabIssueFilter = 'opened' | 'assigned-to-me'

export const GITLAB_MR_FILTERS: { id: GitLabTaskFilter; label: string }[] = [
  { id: 'opened', label: 'Open' },
  { id: 'merged', label: 'Merged' },
  { id: 'closed', label: 'Closed' },
  { id: 'all', label: 'All' }
]

export const GITLAB_ISSUE_FILTERS: { id: GitLabIssueFilter; label: string }[] = [
  { id: 'opened', label: 'Open' },
  { id: 'assigned-to-me', label: 'Assigned to me' }
]

export function isGitLabMRFilter(value: GitLabTaskFilter | GitLabIssueFilter): value is GitLabTaskFilter {
  return value === 'opened' || value === 'merged' || value === 'closed' || value === 'all'
}

export function isGitLabIssueFilter(
  value: GitLabTaskFilter | GitLabIssueFilter
): value is GitLabIssueFilter {
  return value === 'opened' || value === 'assigned-to-me'
}
