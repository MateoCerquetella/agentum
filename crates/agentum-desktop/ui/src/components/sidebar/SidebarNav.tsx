import React from 'react'
import { FolderGit2, Radar, Search, type LucideIcon } from 'lucide-react'
import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import type { GlobalSettings } from '@/shared/types'
import { useActivityUnreadCount } from '@/components/activity/useActivityUnreadCount'
import { useShortcutLabel } from '@/hooks/useShortcutLabel'
import { requestOperationalSidebarSearchFocus } from '@/lib/operational-sidebar-search-focus'

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

export function SidebarNav(): React.JSX.Element {
  const worktreePaletteShortcut = useShortcutLabel('worktree.palette')
  const openActivityPage = useAppStore((s) => s.openActivityPage)
  const openProjectsPage = useAppStore((s) => s.openProjectsPage)
  const openModal = useAppStore((s) => s.openModal)
  const activeView = useAppStore((s) => s.activeView)
  const groupBy = useAppStore((s) => s.groupBy)

  const activityActive = activeView === 'activity'
  // The hub ('project') is a destination *inside* Projects, so the rail item
  // stays lit while the user is in either — one section, two depths.
  const projectsActive = activeView === 'projects' || activeView === 'project'
  // Why: Mission Control is now the always-on home, so its "needs you" badge is
  // always tracked (no longer gated behind the old experimental Agents button).
  const activityUnreadCount = useActivityUnreadCount(true, 'sidebar-badge')

  return (
    <div className="flex flex-col gap-0.5 px-2 pt-2 pb-1">
      {/* Primary workflow rail (Phase 1 nav shell, #48): Mission Control →
          Projects. Settings lives in the bottom toolbar. The Board is
          no longer a rail entry (spec 016): it lives inside each project's
          hub (Projects → project → Tasks). */}
      <PrimaryNavItem
        icon={Radar}
        label="Mission Control"
        active={activityActive}
        onClick={openActivityPage}
        badge={activityUnreadCount}
      />
      {/* Projects replaces both the old global Wiki rail item (spec 009 D1)
          and the v0.59.0 per-repo sidebar group (#274 — Mateo: repos never
          list in the sidebar). It opens the full Projects page; a project's
          surfaces (Chat / Wiki / Board / Sessions) are chosen inside the hub. */}
      <PrimaryNavItem
        icon={FolderGit2}
        label="Projects"
        active={projectsActive}
        onClick={openProjectsPage}
      />

      {/* Secondary utility: fuzzy search. */}
      <button
        type="button"
        onClick={() => {
          if (groupBy === 'operational') {
            requestOperationalSidebarSearchFocus()
          } else {
            openModal('worktree-palette')
          }
        }}
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
}

export default React.memo(SidebarNav)
