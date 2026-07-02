// Project Hub (ADE redesign, "Agentum ADE Prototype"): clicking a project in
// the sidebar opens THIS view — the project's Chat / Wiki / Tasks / Sessions
// as tabs under one header — instead of only expanding the sidebar group. Each
// tab embeds the existing full-page surface pinned to the hub's repo (Chat's
// workspace picker, Wiki's projects rail, and the Board's repo filter collapse
// into the hub's single project scope), so the hub adds navigation, not a
// parallel implementation. The rail's Chat / Wiki / Board entries stay the
// global, cross-project views.
import React, { lazy, Suspense, useEffect, useMemo } from 'react'
import { ChevronLeft } from 'lucide-react'

import { useAppStore } from '@/store'
import type { AppState } from '@/store/types'
import { useActiveRepo, useWorktreesForRepo } from '@/store/selectors'
import { selectServerWorktreeActivity } from '@/store/slices/server-worktree-activity'
import { cn } from '@/lib/utils'
import { getTaskPresetQuery, PER_REPO_FETCH_LIMIT } from '@/lib/new-workspace'
import { isGitRepoKind } from '@/shared/repo-kind'
import { RepoIconGlyph } from '@/components/repo/repo-icon'
import { ProjectSessionsList } from './ProjectSessionsList'

// Lazy like App.tsx's page mounts: the hub chunk stays small and each surface
// loads on first tab visit (Chat/Wiki/TaskPage are already split chunks).
const ChatPage = lazy(() => import('@/components/harness/ChatPage'))
const WikiPage = lazy(() => import('@/components/wiki/WikiPage'))
const TaskPage = lazy(() => import('@/components/TaskPage'))

type HubTab = 'chat' | 'wiki' | 'tasks' | 'sessions'

// The GitHub work-items cache key the Board reads on mount — the exact recipe
// openTaskPage uses for its warm-up prefetch (resume-state custom query wins,
// else the resume/default preset). Reading the same key means the hub's Tasks
// badge always counts what the Tasks tab will actually show.
function boardWorkItemsQuery(s: AppState): string {
  const resume = s.taskResumeState
  return resume?.githubItemsPreset === null
    ? (resume.githubItemsQuery ?? '').trim()
    : getTaskPresetQuery(resume?.githubItemsPreset ?? s.settings?.defaultTaskViewPreset ?? 'all')
}

const TABS: Array<{ id: HubTab; label: string }> = [
  { id: 'chat', label: 'Chat' },
  { id: 'wiki', label: 'Wiki' },
  { id: 'tasks', label: 'Tasks' },
  { id: 'sessions', label: 'Sessions' }
]

export default function ProjectHubPage(): React.JSX.Element {
  const repo = useActiveRepo()
  const tab = useAppStore((s) => s.projectHubTab)
  const setProjectHubTab = useAppStore((s) => s.setProjectHubTab)
  const closeProjectHub = useAppStore((s) => s.closeProjectHub)

  // The embedded TaskPage seeds its repo selection from taskPageData at MOUNT,
  // and a detour through the global Board (rail click, palette) wipes that
  // data (openTaskPage replaces it; closeTaskPage clears it). Re-assert the
  // hub's repo before letting TaskPage mount — the render gate below holds the
  // tab back for the one frame the effect needs, so it can never seed from
  // stale data and silently show every repo's issues.
  const taskDataSeeded = useAppStore((s) =>
    repo != null && s.taskPageData.preselectedRepoId === repo.id
  )
  useEffect(() => {
    if (!repo || tab !== 'tasks' || taskDataSeeded) return
    useAppStore.setState((s) => ({
      taskPageData: { ...s.taskPageData, preselectedRepoId: repo.id }
    }))
  }, [repo, tab, taskDataSeeded])

  // Tasks-tab badge (ADE prototype "Tasks <count>"): open-item count from the
  // work-items cache. Null (no badge) until the key is warm — the prefetch
  // below usually makes it so by the time the header paints.
  const taskCount = useAppStore((s) => {
    if (!repo || !isGitRepoKind(repo)) return null
    return (
      s.getCachedWorkItems(repo.id, PER_REPO_FETCH_LIMIT, boardWorkItemsQuery(s))?.length ?? null
    )
  })
  useEffect(() => {
    if (!repo?.path || !isGitRepoKind(repo)) return
    const s = useAppStore.getState()
    // GitHub only — mirroring the rail's prefetch gate; for Linear/GitLab
    // defaults the badge simply stays absent rather than firing gh for data
    // the user's Tasks tab doesn't lead with.
    if ((s.settings?.defaultTaskSource ?? 'github') !== 'github') return
    s.prefetchWorkItems(repo.id, repo.path, PER_REPO_FETCH_LIMIT, boardWorkItemsQuery(s))
  }, [repo])

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
          <h1 className="truncate text-[14px] font-semibold tracking-tight">{repo.displayName}</h1>
        </div>
        <nav className="ml-3 flex items-center gap-0.5" aria-label="Project sections">
          {TABS.map(({ id, label }) => {
            const isActive = tab === id
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
                {id === 'tasks' && taskCount != null ? (
                  <span className="rounded-full bg-foreground/10 px-1.5 py-px font-mono text-[10px] leading-normal">
                    {taskCount}
                  </span>
                ) : null}
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
              so mount-time scoping (TaskPage's repo preselection, Chat's
              transcript state) re-seeds instead of leaking across projects. */}
          {tab === 'chat' ? <ChatPage key={repo.id} pinnedRepo={repo} /> : null}
          {tab === 'wiki' ? <WikiPage key={repo.id} pinnedRepoId={repo.id} /> : null}
          {tab === 'tasks' && taskDataSeeded ? <TaskPage key={repo.id} embedded /> : null}
          {tab === 'sessions' ? <ProjectSessionsList repoId={repo.id} /> : null}
        </Suspense>
      </div>
    </div>
  )
}
