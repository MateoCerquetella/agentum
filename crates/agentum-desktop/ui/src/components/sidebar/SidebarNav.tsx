import React from 'react'
import {
  BookText,
  Columns3,
  MessagesSquare,
  Radar,
  Search,
  type LucideIcon
} from 'lucide-react'
import { useAppStore } from '@/store'
import { useRepoMap } from '@/store/selectors'
import { cn } from '@/lib/utils'
import { isGitRepoKind } from '../../../../shared/repo-kind'
import type { GlobalSettings } from '../../../../shared/types'
import { getTaskPresetQuery, PER_REPO_FETCH_LIMIT } from '@/lib/new-workspace'
import {
  normalizeVisibleTaskProviders,
  restoreAvailableDefaultTaskProvider,
  resolveVisibleTaskProvider
} from '../../../../shared/task-providers'
import { useActivityUnreadCount } from '@/components/activity/useActivityUnreadCount'
import { useShortcutLabel } from '@/hooks/useShortcutLabel'

export function shouldShowAgentsButton(
  settings: Pick<GlobalSettings, 'experimentalActivity'> | null | undefined
): boolean {
  return settings?.experimentalActivity === true
}

// Shared icon color tokens for the primary nav rail. Every icon rests at one
// muted-monochrome color and only the active destination gets the accent, so the
// rail reads as a single system instead of scattered per-item opacities (Board and
// Search used to hardcode /30 while the primary items used /40 — color noise that
// made colorless entries look broken). Both tokens are backed by theme variables
// (--sidebar-foreground / --sidebar-accent-foreground), never a hardcoded hex, so
// contrast holds in light and dark. Driven off the caller's active-view state.
const NAV_ICON_MUTED = 'text-sidebar-foreground/40'
const NAV_ICON_ACCENT = 'text-sidebar-accent-foreground'

export function navIconClass(active: boolean): string {
  return active ? NAV_ICON_ACCENT : NAV_ICON_MUTED
}

/**
 * A primary workflow rail item (Phase 1 nav shell, #48): icon + plain label + a
 * one-line description, so every destination explains itself instead of relying
 * on a tooltip the user has to discover ("nothing is explained" was the bug).
 */
function PrimaryNavItem({
  icon: Icon,
  label,
  active,
  onClick,
  badge,
  soon
}: {
  icon: LucideIcon
  label: string
  active: boolean
  onClick: () => void
  badge?: number
  soon?: boolean
}): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? 'page' : undefined}
      className={cn(
        'flex w-full items-start gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors',
        active
          ? 'bg-sidebar-accent text-sidebar-accent-foreground'
          : 'text-sidebar-foreground/70 hover:bg-sidebar-foreground/8'
      )}
    >
      <Icon
        className={cn('mt-0.5 size-4 shrink-0', navIconClass(active))}
        strokeWidth={active ? 2.25 : 1.75}
      />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="flex items-center gap-1.5">
          <span className="text-[13px] font-medium tracking-tight">{label}</span>
          {soon ? (
            <span className="rounded-full border border-sidebar-foreground/20 px-1.5 py-px text-[9px] font-semibold uppercase tracking-wide text-sidebar-foreground/50">
              Soon
            </span>
          ) : null}
          {badge && badge > 0 ? (
            <span className="ml-auto rounded-full bg-primary px-1.5 py-px text-[10px] font-semibold text-primary-foreground">
              {badge}
            </span>
          ) : null}
        </span>
      </span>
    </button>
  )
}

const SidebarNav = React.memo(function SidebarNav() {
  const worktreePaletteShortcut = useShortcutLabel('worktree.palette')
  const openTaskPage = useAppStore((s) => s.openTaskPage)
  const openActivityPage = useAppStore((s) => s.openActivityPage)
  const openHarnessPage = useAppStore((s) => s.openHarnessPage)
  const openWikiPage = useAppStore((s) => s.openWikiPage)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const openModal = useAppStore((s) => s.openModal)
  const activeView = useAppStore((s) => s.activeView)
  const repos = useAppStore((s) => s.repos)
  const repoMap = useRepoMap()
  const canBrowseTasks = repos.some((repo) => isGitRepoKind(repo))
  // Why: the setting is opt-out (default true). `!== false` keeps the button
  // visible for users whose persisted settings predate this field.
  const showTasksButton = useAppStore((s) => s.settings?.showTasksButton !== false)
  const rawVisibleTaskProviders = useAppStore((s) => s.settings?.visibleTaskProviders)
  const defaultTaskSource = useAppStore((s) => s.settings?.defaultTaskSource ?? 'github')
  const preflightStatus = useAppStore((s) => s.preflightStatus)
  const preflightStatusChecked = useAppStore((s) => s.preflightStatusChecked)
  const refreshPreflightStatus = useAppStore((s) => s.refreshPreflightStatus)
  const linearStatus = useAppStore((s) => s.linearStatus)
  const linearStatusChecked = useAppStore((s) => s.linearStatusChecked)
  const checkLinearConnection = useAppStore((s) => s.checkLinearConnection)
  const preferredVisibleTaskProviders = React.useMemo(
    () => normalizeVisibleTaskProviders(rawVisibleTaskProviders),
    [rawVisibleTaskProviders]
  )
  const visibleTaskProviders = React.useMemo(
    () =>
      restoreAvailableDefaultTaskProvider(
        preferredVisibleTaskProviders,
        {
          gitlabInstalled: preflightStatus?.glab?.installed === true,
          linearConnected: linearStatus.connected === true
        },
        defaultTaskSource
      ),
    [
      defaultTaskSource,
      linearStatus.connected,
      preferredVisibleTaskProviders,
      preflightStatus?.glab?.installed
    ]
  )
  const resolvedDefaultTaskSource = React.useMemo(
    () => resolveVisibleTaskProvider(defaultTaskSource, visibleTaskProviders),
    [defaultTaskSource, visibleTaskProviders]
  )

  React.useEffect(() => {
    if (!preflightStatusChecked) {
      void refreshPreflightStatus()
    }
    if (!linearStatusChecked) {
      void checkLinearConnection()
    }
  }, [checkLinearConnection, linearStatusChecked, preflightStatusChecked, refreshPreflightStatus])

  // Why: warm the GitHub work-item cache on hover/focus so by the time the
  // user's click finishes the round-trip has either completed or is already
  // in-flight. Shaves ~200–600ms off perceived page-load latency.
  const prefetchWorkItems = useAppStore((s) => s.prefetchWorkItems)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const defaultTaskViewPreset = useAppStore((s) => s.settings?.defaultTaskViewPreset ?? 'all')
  const handlePrefetch = React.useCallback(() => {
    if (!canBrowseTasks || resolvedDefaultTaskSource !== 'github') {
      return
    }
    const activeRepo = activeRepoId ? (repoMap.get(activeRepoId) ?? null) : null
    const activeGitRepo = activeRepo && isGitRepoKind(activeRepo) ? activeRepo : null
    const firstGitRepo = activeGitRepo ?? repos.find((r) => isGitRepoKind(r))
    if (firstGitRepo?.path) {
      // Why: warm the exact cache key the page will read on mount — must
      // match TaskPage's `initialTaskQuery` derived from the same default
      // preset, otherwise the prefetch lands in a key the page never reads
      // and we pay the full round-trip after click.
      prefetchWorkItems(
        firstGitRepo.id,
        firstGitRepo.path,
        PER_REPO_FETCH_LIMIT,
        getTaskPresetQuery(defaultTaskViewPreset)
      )
    }
  }, [
    activeRepoId,
    canBrowseTasks,
    defaultTaskViewPreset,
    prefetchWorkItems,
    repoMap,
    repos,
    resolvedDefaultTaskSource
  ])

  const tasksActive = activeView === 'tasks'
  const activityActive = activeView === 'activity'
  const harnessActive = activeView === 'harness'
  const wikiActive = activeView === 'wiki'
  // Why: Mission Control is now the always-on home, so its "needs you" badge is
  // always tracked (no longer gated behind the old experimental Agents button).
  const activityUnreadCount = useActivityUnreadCount(true, 'sidebar-badge')

  return (
    <div className="flex flex-col gap-0.5 px-2 pt-2 pb-1">
      {/* Primary workflow rail (Phase 1 nav shell, #48): Mission Control → Chat
          → Board mirrors the spec → board → agent pipeline. Settings lives in
          the bottom toolbar, so the rail reads Home → Chat → Board → Settings
          top-to-bottom. */}
      <PrimaryNavItem
        icon={Radar}
        label="Mission Control"
        active={activityActive}
        onClick={openActivityPage}
        badge={activityUnreadCount}
      />
      <PrimaryNavItem
        icon={MessagesSquare}
        label="Chat"
        active={harnessActive}
        onClick={openHarnessPage}
      />
      <PrimaryNavItem icon={BookText} label="Wiki" active={wikiActive} onClick={openWikiPage} />

      {/* Secondary utilities: external task trackers, the goals pipeline, and
          fuzzy search. Tasks + Goals fold into Board in Phase 2/3. */}
      {showTasksButton ? (
        <button
          type="button"
          onClick={() => {
            if (!canBrowseTasks) {
              return
            }
            openTaskPage()
          }}
          onPointerEnter={handlePrefetch}
          onFocus={handlePrefetch}
          disabled={!canBrowseTasks}
          aria-current={tasksActive ? 'page' : undefined}
          className={cn(
            'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] font-medium tracking-tight transition-colors',
            tasksActive
              ? 'bg-sidebar-accent text-sidebar-accent-foreground'
              : 'text-sidebar-foreground/60 hover:bg-sidebar-foreground/8',
            !canBrowseTasks && 'cursor-not-allowed opacity-50 hover:bg-transparent'
          )}
        >
          {/* "Board" = the Tasks view: your GitHub/Linear issues. Chat creates
              issues that show up here. (Tasks renamed to Board, #48 redo.) */}
          <Columns3
            className={cn('size-4 shrink-0', navIconClass(tasksActive))}
            strokeWidth={tasksActive ? 2.25 : 1.75}
          />
          <span className="flex-1">Board</span>
        </button>
      ) : null}
      <button
        type="button"
        onClick={() => openModal('worktree-palette')}
        aria-label="Search worktrees and browser tabs"
        className="group flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] font-medium tracking-tight text-sidebar-foreground/60 transition-colors hover:bg-sidebar-foreground/8"
      >
        {/* Search opens the palette modal, never a persistent view, so it always
            rests at the muted token — accent is reserved for the active destination. */}
        <Search className={cn('size-4 shrink-0', navIconClass(false))} strokeWidth={1.75} />
        <span className="flex-1">Search</span>
        <kbd className="hidden rounded border border-border/60 bg-background/40 px-1.5 py-px font-mono text-[10px] font-medium text-muted-foreground group-hover:inline-flex items-center">
          {worktreePaletteShortcut}
        </kbd>
      </button>
    </div>
  )
})

export default SidebarNav
