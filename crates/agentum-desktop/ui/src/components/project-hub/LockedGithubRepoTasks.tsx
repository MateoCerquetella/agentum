import { useCallback, useEffect, useMemo, useState } from 'react'
import { CheckCircle2, RefreshCw, Search } from 'lucide-react'
import type { Repo } from '@/shared/types'
import type { GithubRepositoryTaskScope } from '@/lib/project-task-scope'
import { captureProjectTaskScopeGuard } from '@/lib/project-task-scope-guard'
import { isLiveProjectTaskScopeAuthority } from '@/lib/project-task-scope-authority'
import { getLinkedWorkItemSuggestedName } from '@/lib/new-workspace'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

const ISSUE_LIMIT = 100

export function LockedGithubRepoTasks({
  repo,
  scope
}: {
  repo: Repo
  scope: GithubRepositoryTaskScope
}): React.JSX.Element {
  const fetchWorkItems = useAppStore((state) => state.fetchWorkItems)
  const openModal = useAppStore((state) => state.openModal)
  const [items, setItems] = useState(() =>
    (useAppStore.getState().getCachedWorkItems(repo.id, ISSUE_LIMIT, '') ?? []).filter(
      (item) => item.type === 'issue'
    )
  )
  const [loading, setLoading] = useState(items.length === 0)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')

  const refresh = useCallback(
    async (force = false): Promise<void> => {
      const guard = captureProjectTaskScopeGuard(scope)
      if (!guard || !isLiveProjectTaskScopeAuthority(guard)) return
      setLoading(true)
      setError(null)
      try {
        const result = await fetchWorkItems(repo.id, repo.path, ISSUE_LIMIT, '', { force })
        if (!isLiveProjectTaskScopeAuthority(guard)) return
        setItems(result.filter((item) => item.type === 'issue'))
      } catch (cause) {
        if (isLiveProjectTaskScopeAuthority(guard)) {
          setError(cause instanceof Error ? cause.message : 'Could not load GitHub issues.')
        }
      } finally {
        if (isLiveProjectTaskScopeAuthority(guard)) setLoading(false)
      }
    },
    [fetchWorkItems, repo.id, repo.path, scope]
  )

  useEffect(() => {
    setQuery('')
    void refresh()
  }, [refresh, scope.scopeKey, scope.generation])

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return items
    return items.filter(
      (item) =>
        item.title.toLowerCase().includes(needle) ||
        `#${item.number}`.includes(needle.replace(/^#?/, '#'))
    )
  }, [items, query])

  const startWorkspace = useCallback(
    (item: (typeof items)[number]): void => {
      const guard = captureProjectTaskScopeGuard(scope)
      if (!guard || !isLiveProjectTaskScopeAuthority(guard)) return
      openModal('new-workspace-composer', {
        linkedWorkItem: {
          type: 'issue',
          number: item.number,
          title: item.title,
          url: item.url
        },
        prefilledName: getLinkedWorkItemSuggestedName(item),
        initialRepoId: repo.id,
        telemetrySource: 'sidebar',
        requiredProjectTaskScope: guard
      })
    },
    [openModal, repo.id, scope]
  )

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <div className="flex flex-col gap-2 border-b border-border/70 p-3 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-semibold">Repository issues</p>
          <p className="truncate font-mono text-[10.5px] text-muted-foreground">
            {scope.repoSlug} · no GitHub Project bound
          </p>
        </div>
        <label className="flex h-8 min-w-0 items-center gap-2 rounded-md border border-input bg-background px-2 sm:w-64">
          <Search className="size-3.5 flex-none text-muted-foreground" />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-label="Filter repository issues"
            placeholder="Filter title or #number"
            className="min-w-0 flex-1 bg-transparent text-xs outline-none placeholder:text-muted-foreground"
          />
        </label>
        <Button
          size="sm"
          variant="outline"
          disabled={loading}
          onClick={() => void refresh(true)}
        >
          <RefreshCw className={cn('mr-1.5 size-3.5', loading && 'animate-spin')} />
          Refresh
        </Button>
      </div>
      {error ? (
        <p role="alert" className="border-b border-border/70 p-3 text-xs text-destructive">
          {error}
        </p>
      ) : null}
      <div className="min-h-0 flex-1 overflow-auto p-3">
        {loading && items.length === 0 ? (
          <p className="text-sm text-muted-foreground">Loading repository issues…</p>
        ) : visible.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {query.trim() ? 'No open issues match this filter.' : 'No open repository issues.'}
          </p>
        ) : (
          <ul className="space-y-2">
            {visible.map((item) => (
              <li
                key={item.id}
                className="flex flex-col gap-2 rounded-md border border-border/70 p-3 sm:flex-row sm:items-center"
              >
                <CheckCircle2 className="size-3.5 flex-none text-muted-foreground" />
                <span className="font-mono text-xs text-muted-foreground">#{item.number}</span>
                <span className="min-w-0 flex-1 truncate text-sm">{item.title}</span>
                <Button size="sm" variant="outline" onClick={() => startWorkspace(item)}>
                  Start workspace
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  )
}
