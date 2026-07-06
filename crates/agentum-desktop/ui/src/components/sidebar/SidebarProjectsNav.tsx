import React from 'react'
import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { RepoIconGlyph } from '@/components/repo/repo-icon'
import { projectsNavRows } from './projects-nav-rows'

/**
 * The sidebar "Projects" group (spec 009 F1, D-A1/D2): one compact row per
 * registered repo, each opening that project's hub (Chat / Wiki / Tasks /
 * Sessions). This replaced the global Wiki rail item — a wiki belongs to one
 * project, so the hub's Wiki tab is now the only wiki surface.
 *
 * Deliberately a separate, always-visible section (not hung off
 * `groupBy === 'repo'`, which is user-toggleable) and NOT inside SidebarNav,
 * which is memo'd around a stable prop-less primary-destinations contract.
 *
 * D-A8: this section deliberately IGNORES `filterRepoIds` — the sidebar filter
 * governs which *workspace rows* show, not project access. A repo filtered out
 * of the workspace list must still be openable as a project (its hub is the
 * only wiki path). Do not wire the filter in without a PM change.
 *
 * No worktree rows, no status dots, no counts: worktrees are WorktreeList's
 * job, and per-repo wiki-status dots would reintroduce the N×`GET /api/wiki`
 * sweep this spec exists to kill (arch §5.6).
 */
const SidebarProjectsNav = React.memo(function SidebarProjectsNav() {
  const repos = useAppStore((s) => s.repos)
  const activeView = useAppStore((s) => s.activeView)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const openProjectHub = useAppStore((s) => s.openProjectHub)

  if (repos.length === 0) {
    // Nothing to list — the add-project affordances live elsewhere in the
    // sidebar, so an empty labeled group would be dead chrome.
    return null
  }

  const rows = projectsNavRows(repos, activeView, activeRepoId)
  const repoById = new Map(repos.map((r) => [r.id, r]))

  return (
    <div className="flex flex-col px-2 pb-1">
      <div className="px-2 pb-0.5 pt-1 text-[11px] font-medium uppercase tracking-wide text-sidebar-foreground/50">
        Projects
      </div>
      {/* Bounded height: this section sits in the sidebar's fixed (non-scrolling)
          region above WorktreeList — many repos must not push workspaces
          off-screen, so the list scrolls internally instead. */}
      <ul className="flex max-h-40 flex-col gap-0.5 overflow-y-auto">
        {rows.map((row) => {
          const repo = repoById.get(row.id)
          return (
            <li key={row.id}>
              <button
                type="button"
                onClick={() => openProjectHub(row.id)}
                aria-current={row.active ? 'page' : undefined}
                className={cn(
                  'flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-[13px] font-medium tracking-tight transition-colors',
                  row.active
                    ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                    : 'text-sidebar-foreground/70 hover:bg-sidebar-foreground/8'
                )}
              >
                <RepoIconGlyph
                  repoIcon={repo?.repoIcon}
                  className="size-4 shrink-0"
                  iconClassName="size-3.5"
                />
                <span className="truncate">{row.label}</span>
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
})

export default SidebarProjectsNav
