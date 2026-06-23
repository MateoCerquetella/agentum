// Two-way push-back for the GitHub Kanban: when a card is dragged to a new
// column, optimistically move it in the store cache and PATCH the issue state
// on GitHub, rolling back on failure. Mirrors the inline logic in
// TaskPage's GHStatusCell so both paths behave identically; lives here so the
// 8k-line TaskPage only needs a one-line call from the board's onMove.
import { toast } from 'sonner'

import { useAppStore } from '@/store'
import { api } from '@/tauri'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import type { GitHubWorkItem, Repo } from '@/shared/types'

import { githubTargetState, type KanbanColumnKey } from './task-kanban'

/**
 * Transition a GitHub *issue* to the state implied by `target` (Done→closed,
 * else→open). No-op for PRs (their close/merge semantics differ), when no repo
 * is resolved, or when the state wouldn't change. Optimistic + rollback —
 * `filteredWorkItems` derives from the same store cache, so the card moves
 * columns immediately and snaps back if the API rejects.
 */
export function transitionGithubIssue(
  item: GitHubWorkItem,
  repo: Repo | null,
  target: KanbanColumnKey
): void {
  if (!repo || item.type !== 'issue') return
  const newState = githubTargetState(target)
  if (newState === item.state) return

  const store = useAppStore.getState()
  store.patchWorkItem(item.id, { state: newState }, item.repoId) // optimistic

  const rollback = (): void => {
    useAppStore.getState().patchWorkItem(item.id, { state: item.state }, item.repoId)
  }

  const rtTarget = getActiveRuntimeTarget(store.settings)
  const updatePromise =
    rtTarget.kind === 'environment'
      ? callRuntimeRpc<{ ok?: boolean; error?: string }>(
          rtTarget,
          'github.updateIssue',
          { repo: repo.id, number: item.number, updates: { state: newState } },
          { timeoutMs: 30_000 }
        )
      : api.gh.updateIssue({
          repoPath: repo.path,
          repoId: repo.id,
          number: item.number,
          updates: { state: newState }
        })

  void Promise.resolve(updatePromise)
    .then((result) => {
      const typed = result as { ok?: boolean; error?: string }
      if (typed && typed.ok === false) {
        rollback()
        toast.error(typed.error ?? 'Failed to update issue state')
      }
    })
    .catch((e: unknown) => {
      rollback()
      toast.error(e instanceof Error ? e.message : 'Failed to update issue state')
    })
}
