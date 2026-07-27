// Project Hub (ADE redesign, "Agentum ADE Prototype"): clicking a project in
// the sidebar opens THIS view — the project's Specs / Wiki / Tasks / Sessions
// as tabs under one header — instead of only expanding the sidebar group. Each
// tab embeds the existing full-page surface pinned to the hub's repo (Run
// Center, Wiki's projects rail, and the Tasks repo filter collapse
// into the hub's single project scope), so the hub adds navigation, not a
// parallel implementation.
import React, { lazy, Suspense, useMemo } from 'react'
import { ChevronLeft } from 'lucide-react'

import { useAppStore } from '@/store'
import { useActiveRepo, useWorktreesForRepo } from '@/store/selectors'
import { selectServerWorktreeActivity } from '@/store/slices/server-worktree-activity'
import { cn } from '@/lib/utils'
import { RepoIconGlyph } from '@/components/repo/repo-icon'
import { RepoBadgeMark } from '@/components/repo/RepoBadgeLabel'
import { ProjectSessionsList } from './ProjectSessionsList'
import { ProjectTasksPage } from './ProjectTasksPage'
import SddWorkspaceBar from '@/components/sdd/SddWorkspaceBar'

// Lazy like App.tsx's page mounts: the hub chunk stays small and each surface
// loads on first tab visit (Wiki/TaskPage are already split chunks).
const WikiPage = lazy(() => import('@/components/wiki/WikiPage'))

type HubTab = 'specs' | 'wiki' | 'tasks' | 'tracker' | 'sessions'

const TABS: Array<{ id: HubTab; label: string }> = [
  { id: 'specs', label: 'Specs' },
  { id: 'wiki', label: 'Wiki' },
  // #379 (Mateo): Tracker and Tasks are ONE surface — the board binding +
  // intake now live in a collapsible strip atop the Tasks tab. The 'tracker'
  // tab id survives in the store union for deep-link compat; it lands on
  // Tasks with the strip expanded.
  { id: 'tasks', label: 'Tasks' },
  { id: 'sessions', label: 'Sessions' }
]

export default function ProjectHubPage(): React.JSX.Element {
  const repo = useActiveRepo()
  const tab = useAppStore((s) => s.projectHubTab)
  const setProjectHubTab = useAppStore((s) => s.setProjectHubTab)
  const closeProjectHub = useAppStore((s) => s.closeProjectHub)

  const worktrees = useWorktreesForRepo(repo?.id ?? null)
  const visibleWorktrees = useMemo(() => worktrees.filter((w) => !w.isArchived), [worktrees])
  // Server-authoritative "running" rollup for the Sessions tab badge — the same
  // source the sidebar dots use, so the two never disagree.
  const runningCount = useAppStore((s) =>
    visibleWorktrees.reduce(
      (n, w) => n + (selectServerWorktreeActivity(s, w.id).liveActivity === 'working' ? 1 : 0),
      0
    )
  )

  if (!repo) {
    // The active repo was removed out from under the hub (project deleted) —
    // bail to the previous view rather than rendering an unusable shell.
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 bg-background">
        <div className="text-sm text-muted-foreground">This project is no longer available.</div>
        <button
          type="button"
          onClick={closeProjectHub}
          className="rounded-md border border-border bg-card px-3 py-1.5 text-[12.5px] font-medium hover:bg-accent"
        >
          Go back
        </button>
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex h-12 flex-none items-center gap-2 border-b border-border px-3">
        <button
          type="button"
          onClick={closeProjectHub}
          aria-label="Close project"
          className="inline-flex shrink-0 items-center rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
        >
          <ChevronLeft className="size-4" />
        </button>
        <div className="flex min-w-0 items-center gap-2">
          <RepoIconGlyph
            repoIcon={repo.repoIcon}
            color={repo.badgeColor}
            className="size-4 shrink-0 text-muted-foreground"
            iconClassName="size-3.5"
          />
          <RepoBadgeMark color={repo.badgeColor} className="size-1.5 rounded-full" />
          <h1 className="truncate text-[14px] font-semibold tracking-tight">{repo.displayName}</h1>
        </div>
        <nav className="ml-3 flex items-center gap-0.5" aria-label="Project sections">
          {TABS.map(({ id, label }) => {
            const isActive = (tab === 'tracker' ? 'tasks' : tab) === id
            return (
              <button
                key={id}
                type="button"
                onClick={() => setProjectHubTab(id)}
                aria-current={isActive ? 'page' : undefined}
                className={cn(
                  'inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[12.5px] transition-colors',
                  isActive
                    ? 'bg-accent font-semibold text-foreground'
                    : 'text-muted-foreground hover:bg-foreground/5 hover:text-foreground'
                )}
              >
                {label}
                {id === 'sessions' ? (
                  <span
                    className={cn(
                      'font-mono text-[10.5px]',
                      runningCount > 0 ? 'text-chart-3' : 'text-muted-foreground/70'
                    )}
                  >
                    {runningCount > 0 ? `● ${runningCount}` : visibleWorktrees.length}
                  </span>
                ) : null}
              </button>
            )
          })}
        </nav>
      </header>

      <div className="min-h-0 flex-1">
        <Suspense fallback={null}>
          {/* key={repo.id} remounts each surface when the hub switches projects
              so mount-time scoping cannot leak across repositories. */}
          {tab === 'specs' ? <SddWorkspaceBar key={repo.id} repoId={repo.id} projectName={repo.displayName} presentation="page" initiallyExpanded /> : null}
          {tab === 'wiki' ? <WikiPage key={repo.id} pinnedRepoId={repo.id} /> : null}
          {tab === 'tasks' || tab === 'tracker' ? <ProjectTasksPage key={repo.id} repo={repo} /> : null}
          {tab === 'sessions' ? <ProjectSessionsList repoId={repo.id} /> : null}
        </Suspense>
      </div>
    </div>
  )
}
