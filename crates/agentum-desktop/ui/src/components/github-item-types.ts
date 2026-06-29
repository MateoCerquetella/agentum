/** Why: Project-origin rows don't always belong to the active local repo.
 *  When set, GHEditSection routes label/assignee/state mutations through
 *  slug-addressed IPCs against `owner`/`repo` instead of through `repoPath`,
 *  preventing edits from silently landing on the workspace's repo when the
 *  Project view is showing rows from a different repo. See
 *  docs/design/github-project-view-tasks.md §Dialog editing from Project rows.
 */
export type GitHubItemDialogProjectOrigin = {
  owner: string
  repo: string
  number: number
  type: 'issue' | 'pr'
  projectId: string
  projectItemId: string
  cacheKey: string
}
