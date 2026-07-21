import React, { useMemo, useState } from 'react'
import { FolderGit2, Search } from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { Input } from '@/components/ui/input'
import { RepoIconGlyph } from '@/components/repo/repo-icon'
import type { Repo } from '@/shared/types'
import {
  filterProjectsRows,
  groupProjectsRowsByHost,
  projectsPageRows,
  type ProjectsPageRow
} from './projects-page-rows'

/**
 * The Projects page (#274): the ONE project entry point. Mateo's design rule —
 * the sidebar never lists repos (the v0.59.0 SidebarProjectsNav group was
 * reverted); you pick a project HERE, then choose its surface (Chat / Wiki /
 * Board / Sessions) inside the Project Hub this page opens.
 *
 * #279: a case-insensitive name filter narrows the grid, and projects are
 * grouped under their host (Local first, then each SSH remote) so a specific
 * project is easy to find; host sections with no matches don't render.
 */
export default function ProjectsPage(): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  const worktreesByRepo = useAppStore((s) => s.worktreesByRepo)
  const sshTargetLabels = useAppStore((s) => s.sshTargetLabels)
  const openProjectHub = useAppStore((s) => s.openProjectHub)

  const [query, setQuery] = useState('')

  const rows = useMemo(() => {
    const counts: Record<string, number> = {}
    for (const [repoId, worktrees] of Object.entries(worktreesByRepo)) {
      counts[repoId] = worktrees.length
    }
    return projectsPageRows(repos, counts, sshTargetLabels)
  }, [repos, worktreesByRepo, sshTargetLabels])

  const repoById = useMemo(() => new Map(repos.map((r) => [r.id, r])), [repos])

  const groups = useMemo(
    () => groupProjectsRowsByHost(filterProjectsRows(rows, query)),
    [rows, query]
  )

  const hasProjects = rows.length > 0
  const hasMatches = groups.length > 0

  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto bg-background">
      <div className="mx-auto w-full max-w-5xl px-8 py-10">
        <h1 className="text-xl font-semibold tracking-tight text-foreground">Projects</h1>
        <p className="mt-1 text-[13px] text-muted-foreground">
          Pick a project — its Chat, Wiki, Board and Sessions live inside.
        </p>

        {!hasProjects ? (
          <div className="mt-16 flex flex-col items-center gap-2 text-center">
            <FolderGit2 className="size-8 text-muted-foreground/50" />
            <div className="text-sm font-medium text-foreground">No projects yet</div>
            <p className="max-w-sm text-[13px] text-muted-foreground">
              Add a repository from the sidebar and it will show up here as a project.
            </p>
          </div>
        ) : (
          <>
            <div className="relative mt-6">
              <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                type="search"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search projects by name…"
                aria-label="Search projects by name"
                className="pl-9"
              />
            </div>

            {!hasMatches ? (
              <div className="mt-16 flex flex-col items-center gap-2 text-center">
                <Search className="size-8 text-muted-foreground/50" />
                <div className="text-sm font-medium text-foreground">No projects match</div>
                <p className="max-w-sm text-[13px] text-muted-foreground">
                  No project name contains “{query.trim()}”. Try a different search.
                </p>
              </div>
            ) : (
              <div className="mt-6 flex flex-col gap-7">
                {groups.map((group) => (
                  <section key={group.host}>
                    <h2 className="mb-2.5 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                      {group.host}
                    </h2>
                    <ul className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3">
                      {group.rows.map((row) => (
                        <li key={row.id}>
                          <ProjectCard
                            row={row}
                            repo={repoById.get(row.id)}
                            onOpen={() => openProjectHub(row.id)}
                          />
                        </li>
                      ))}
                    </ul>
                  </section>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}

function ProjectCard({
  row,
  repo,
  onOpen
}: {
  row: ProjectsPageRow
  repo: Repo | undefined
  onOpen: () => void
}): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onOpen}
      className={cn(
        'flex w-full flex-col gap-2 rounded-lg border border-border bg-card p-4 text-left',
        'transition-colors hover:border-border/80 hover:bg-accent/50',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring'
      )}
    >
      <div className="flex items-center gap-2.5">
        <RepoIconGlyph repoIcon={repo?.repoIcon} className="size-7 shrink-0" iconClassName="size-5" />
        <span className="truncate text-sm font-medium text-foreground">{row.name}</span>
      </div>
      <span className="truncate text-xs text-muted-foreground" title={row.path}>
        {row.path}
      </span>
      <div className="flex items-center gap-1.5">
        {row.remote ? (
          <span className="rounded border border-border px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
            SSH
          </span>
        ) : null}
        <span className="text-[11px] text-muted-foreground/80">
          {row.worktrees === 1 ? '1 workspace' : `${row.worktrees} workspaces`}
        </span>
      </div>
    </button>
  )
}
