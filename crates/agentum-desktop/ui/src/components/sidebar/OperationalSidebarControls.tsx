import React, { useEffect, useMemo, useRef, useState } from 'react'
import { Plus, Search } from 'lucide-react'
import { useAppStore } from '@/store'
import { useActiveWorktree, useAllWorktrees } from '@/store/selectors'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { useShortcutLabel } from '@/hooks/useShortcutLabel'
import {
  focusOperationalSidebarSearch,
  OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT
} from '@/lib/operational-sidebar-search-focus'
import { cn } from '@/lib/utils'
import SidebarWorkspaceOptionsMenu from './SidebarWorkspaceOptionsMenu'
import {
  orderOperationalProjects,
  visibleOperationalProjectCount
} from './operational-project-overflow'

export function OperationalSidebarControls({
  query,
  onQueryChange,
  boardControl
}: {
  query: string
  onQueryChange: (query: string) => void
  boardControl?: React.ReactNode
}): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  const worktrees = useAllWorktrees()
  const activeWorktree = useActiveWorktree()
  const filterRepoIds = useAppStore((s) => s.filterRepoIds)
  const setFilterRepoIds = useAppStore((s) => s.setFilterRepoIds)
  const openModal = useAppStore((s) => s.openModal)
  const shortcut = useShortcutLabel('worktree.palette')
  const inputRef = useRef<HTMLInputElement>(null)
  const railRef = useRef<HTMLDivElement>(null)
  const [railWidth, setRailWidth] = useState(0)

  useEffect(() => {
    const focus = () => {
      if (focusOperationalSidebarSearch(inputRef.current)) return
      requestAnimationFrame(() => focusOperationalSidebarSearch(inputRef.current))
    }
    window.addEventListener(OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT, focus)
    return () => window.removeEventListener(OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT, focus)
  }, [])

  useEffect(() => {
    const element = railRef.current
    if (!element || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(([entry]) => {
      const width = Math.round(entry?.contentRect.width ?? 0)
      setRailWidth((current) => (current === width ? current : width))
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  const workspaceCountByRepoId = useMemo(() => {
    const counts = new Map<string, number>()
    for (const worktree of worktrees) {
      counts.set(worktree.repoId, (counts.get(worktree.repoId) ?? 0) + 1)
    }
    return counts
  }, [worktrees])
  const orderedRepos = useMemo(
    () =>
      orderOperationalProjects({
        repos,
        selectedRepoIds: filterRepoIds,
        activeRepoId: activeWorktree?.repoId,
        workspaceCountByRepoId
      }),
    [activeWorktree?.repoId, filterRepoIds, repos, workspaceCountByRepoId]
  )
  const visibleCount = visibleOperationalProjectCount({
    availableWidth: railWidth,
    reservedWidth: 76,
    projectWidths: orderedRepos.map((repo) => Math.min(88, 28 + repo.displayName.length * 7))
  })
  const visibleRepos = orderedRepos.slice(0, visibleCount)
  const overflowRepos = orderedRepos.slice(visibleCount)
  const selected = new Set(filterRepoIds)
  const toggleRepo = (id: string) =>
    setFilterRepoIds(selected.has(id) ? filterRepoIds.filter((value) => value !== id) : [...filterRepoIds, id])

  return (
    <div className="border-b border-sidebar-border/70 px-2 pb-2 pt-1.5" data-operational-sidebar-controls="">
      <div className="flex items-center gap-1.5">
        <label className="relative min-w-0 flex-1">
          <Search className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            ref={inputRef}
            type="search"
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
            placeholder="Search workspaces…"
            aria-label="Search workspaces"
            className="h-8 w-full rounded-md border border-sidebar-border bg-background/55 pl-7 pr-10 text-xs text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-sidebar-ring"
          />
          <kbd className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 font-mono text-[9px] text-muted-foreground">
            {shortcut}
          </kbd>
        </label>
        <SidebarWorkspaceOptionsMenu />
        {boardControl}
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="New workspace"
          disabled={repos.length === 0}
          onClick={() => openModal('new-workspace-composer', { telemetrySource: 'sidebar' })}
        >
          <Plus className="size-3.5" />
        </Button>
      </div>
      <div ref={railRef} className="mt-1.5 flex min-w-0 items-center gap-1 overflow-hidden">
        <button
          type="button"
          aria-pressed={filterRepoIds.length === 0}
          onClick={() => setFilterRepoIds([])}
          className={cn(
            'h-6 shrink-0 rounded border px-2 text-[10px] font-medium focus-visible:ring-2 focus-visible:ring-sidebar-ring',
            filterRepoIds.length === 0 ? 'border-sidebar-ring bg-sidebar-accent text-foreground' : 'border-sidebar-border text-muted-foreground'
          )}
        >
          All
        </button>
        {visibleRepos.map((repo) => (
          <button
            key={repo.id}
            type="button"
            aria-pressed={selected.has(repo.id)}
            onClick={() => toggleRepo(repo.id)}
            className={cn(
              'h-6 max-w-[88px] shrink-0 rounded border px-2 text-[10px] font-medium focus-visible:ring-2 focus-visible:ring-sidebar-ring',
              selected.has(repo.id) ? 'border-sidebar-ring bg-sidebar-accent text-foreground' : 'border-sidebar-border text-muted-foreground'
            )}
          >
            <span className="block truncate">{repo.displayName}</span>
          </button>
        ))}
        {overflowRepos.length > 0 ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="xs"
                className="ml-auto h-6 shrink-0 px-1.5 text-[10px] tabular-nums"
                aria-label={`${overflowRepos.length} more projects`}
              >
                +{overflowRepos.length}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="max-h-72 min-w-44 overflow-y-auto">
              {overflowRepos.map((repo) => (
                <DropdownMenuCheckboxItem
                  key={repo.id}
                  checked={selected.has(repo.id)}
                  onCheckedChange={() => toggleRepo(repo.id)}
                >
                  {repo.displayName}
                </DropdownMenuCheckboxItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}
      </div>
    </div>
  )
}
