import { api } from '@/tauri'
/* eslint-disable max-lines -- Why: top-level Project-mode container coordinates picker, view selection, query overrides, fetch lifecycle, and toolbar interactions; splitting these would fragment shared state. */
// Why: top-level container for Project mode. Handles the picker, header,
// filter label, count pill, Open-in-GitHub, and all Interaction States
// documented in the design doc.
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ExternalLink,
  RefreshCw,
  KanbanSquare,
  Map as MapIcon,
  Search,
  Table as TableIcon,
  X
} from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { HoverCard, HoverCardContent, HoverCardTrigger } from '@/components/ui/hover-card'
import GitHubItemDialog, { type GitHubItemDialogProjectOrigin } from '@/components/GitHubItemDialog'
import { GhAuthErrorHelp } from '@/components/github-project/GhAuthErrorHelp'
import { buildGithubIssueLinkedWorkItem } from '@/lib/github-linked-work-item'
import { launchWorkItemDirect } from '@/lib/launch-work-item-direct'
import { getLinkedWorkItemSuggestedName, type LinkedWorkItemSummary } from '@/lib/new-workspace'
import { useRepoSlugIndex } from '@/lib/repo-slug-index'
import { cn } from '@/lib/utils'
import { fetchGithubIssueBody } from '@/runtime/github-issue-client'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { useAppStore } from '@/store'
import { useMountedRef } from '@/hooks/useMountedRef'
import { projectViewCacheKey } from '@/store/slices/github'
import type {
  GetProjectViewTableResult,
  GitHubIssueType,
  GitHubProjectFieldMutationValue,
  GitHubProjectRow,
  GitHubProjectTable,
  GitHubProjectViewError,
  GitHubProjectViewSummary,
  ListProjectViewsResult
} from '../../../../shared/github-project-types'
import type { GitHubWorkItem, Repo } from '../../../../shared/types'
import ProjectPicker, { type ResolvedProjectSelection } from './ProjectPicker'
import ProjectViewList from './ProjectViewList'
import ProjectBoardView from './ProjectBoardView'
import ProjectItemSlugDialog from './ProjectItemSlugDialog'
import { useProjectViewLiveRefresh } from './use-project-view-live-refresh'
import {
  resolveMissingRepoProjectDialogState,
  resolveRepoBackedProjectDialogState
} from './project-dialog-state'
import { classifyStartWorkRepoMatches } from './start-work-repo-match'

type Props = Record<string, never>

const AGENTUM_FEATURE_REQUEST_URL = 'https://github.com/mateocerquetella/agentum/issues/new'

function listProjectViewsForRuntime(
  settings: Parameters<typeof getActiveRuntimeTarget>[0],
  args: { owner: string; ownerType: 'organization' | 'user'; projectNumber: number }
): Promise<ListProjectViewsResult> {
  const target = getActiveRuntimeTarget(settings)
  return target.kind === 'environment'
    ? callRuntimeRpc<ListProjectViewsResult>(target, 'github.project.listViews', args, {
        timeoutMs: 30_000
      })
    : api.gh.listProjectViews(args)
}

export default function ProjectViewWrapper(_props: Props = {} as Props): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const projectViewCache = useAppStore((s) => s.projectViewCache)
  const fetchProjectViewTable = useAppStore((s) => s.fetchProjectViewTable)
  const updateProjectFieldValue = useAppStore((s) => s.updateProjectFieldValue)
  const clearProjectFieldValue = useAppStore((s) => s.clearProjectFieldValue)
  const patchProjectIssueOrPr = useAppStore((s) => s.patchProjectIssueOrPr)
  const patchProjectRowIssueType = useAppStore((s) => s.patchProjectRowIssueType)
  const addRepoFromStore = useAppStore((s) => s.addRepo)
  const repos = useAppStore((s) => s.repos)
  const { lookupSlug, ready: slugIndexReady } = useRepoSlugIndex()
  const mountedRef = useMountedRef()

  const activeProject = settings?.githubProjects?.activeProject ?? null
  const lastViewByProject = useMemo(
    () => settings?.githubProjects?.lastViewByProject ?? {},
    [settings?.githubProjects?.lastViewByProject]
  )

  const [loading, setLoading] = useState(false)
  const fetchRunIdRef = useRef(0)
  const [error, setError] = useState<{
    error: GitHubProjectViewError
    totalCount?: number
  } | null>(null)
  const [parentDroppedToasted, setParentDroppedToasted] = useState<ReadonlySet<string>>(
    () => new Set()
  )
  // Why: cache the project's view list per active project so the tab strip
  // renders without flicker on re-renders and survives view switches without
  // refetching. Keyed by `ownerType:owner:number`.
  const [viewListByProject, setViewListByProject] = useState<
    Record<string, GitHubProjectViewSummary[]>
  >({})

  // Why: ephemeral search override, scoped to (project, view). Mirrors GitHub
  // Projects' search box — pre-populated from `selectedView.filter`, applied
  // on Enter/blur, cleared with the X button. The override is NEVER persisted
  // to settings or to GitHub (per design doc §"Out of scope" line 36); a tab
  // switch or refresh resets to the view's stored filter. Keyed by
  // `ownerType:owner:number:viewId`. `undefined` (entry missing) means
  // "use the view's filter as-is" so the cache key collapses to the
  // unfiltered cache entry. The transient input string lives inside
  // `ProjectSearchInput` so typing does not re-render the table.
  const [appliedQueryByView, setAppliedQueryByView] = useState<Record<string, string>>({})

  const doFetch = useCallback(
    async (selection: ResolvedProjectSelection, force = false, queryOverride?: string) => {
      const runId = fetchRunIdRef.current + 1
      fetchRunIdRef.current = runId
      setLoading(true)
      setError(null)
      try {
        const res: GetProjectViewTableResult = await fetchProjectViewTable(
          {
            owner: selection.owner,
            ownerType: selection.ownerType,
            projectNumber: selection.projectNumber,
            ...(selection.viewId ? { viewId: selection.viewId } : {}),
            ...(queryOverride !== undefined ? { queryOverride } : {})
          },
          { force }
        )
        if (!mountedRef.current || fetchRunIdRef.current !== runId) {
          return
        }
        if (!res.ok) {
          setError({ error: res.error, totalCount: res.totalCount })
        }
      } finally {
        // Why: a manual refresh can overlap with a tab/search fetch; an older
        // request finishing first must not clear the newer refresh indicator.
        if (mountedRef.current && fetchRunIdRef.current === runId) {
          setLoading(false)
        }
      }
    },
    [fetchProjectViewTable, mountedRef]
  )

  const handleSelect = useCallback(
    async (selection: ResolvedProjectSelection) => {
      await doFetch(selection, true)
    },
    [doFetch]
  )

  // Auto-fetch when activeProject exists and we don't have cached data.
  useEffect(() => {
    if (!activeProject) {
      return
    }
    const key = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
    const viewId = lastViewByProject[key]?.viewId
    if (!viewId) {
      return
    }
    const projectViewKey = `${key}:${viewId}`
    const queryOverride = appliedQueryByView[projectViewKey]
    const cacheKey = projectViewCacheKey(
      activeProject.ownerType,
      activeProject.owner,
      activeProject.number,
      viewId,
      queryOverride
    )
    if (projectViewCache[cacheKey]?.data) {
      return
    }
    void doFetch(
      {
        owner: activeProject.owner,
        ownerType: activeProject.ownerType,
        projectNumber: activeProject.number,
        viewId
      },
      false,
      queryOverride
    )
  }, [activeProject, lastViewByProject, projectViewCache, doFetch, appliedQueryByView])

  // Spec 014 F3: a tracker.* event (agentum just moved a card on GitHub)
  // re-fetches the active view after the 2 s coalesce window — same values as
  // the auto-fetch effect above, but force=true (the cache holds the stale
  // pre-move table). Event-driven only; no interval.
  const liveRefetch = useCallback(() => {
    if (!activeProject) {
      return
    }
    const key = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
    const viewId = lastViewByProject[key]?.viewId
    if (!viewId) {
      return
    }
    const queryOverride = appliedQueryByView[`${key}:${viewId}`]
    void doFetch(
      {
        owner: activeProject.owner,
        ownerType: activeProject.ownerType,
        projectNumber: activeProject.number,
        viewId
      },
      /* force */ true,
      queryOverride
    )
  }, [activeProject, lastViewByProject, appliedQueryByView, doFetch])
  useProjectViewLiveRefresh(liveRefetch)

  // Load the project's view list whenever the active project changes so the
  // tab strip can render. The list is small and rarely changes — fetched once
  // per project per session is fine.
  useEffect(() => {
    if (!activeProject) {
      return
    }
    const projectKey = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
    if (viewListByProject[projectKey]) {
      return
    }
    let cancelled = false
    void listProjectViewsForRuntime(settings, {
      owner: activeProject.owner,
      ownerType: activeProject.ownerType,
      projectNumber: activeProject.number
    })
      .then((res) => {
        if (cancelled) {
          return
        }
        if (res.ok) {
          setViewListByProject((prev) => ({ ...prev, [projectKey]: res.views }))
        } else {
          console.warn('[project-view] listProjectViews failed:', res.error.message)
        }
      })
      .catch((err) => {
        if (cancelled) {
          return
        }
        // Why: an IPC rejection here would surface as an unhandled rejection
        // and dev-tools red — log and fall back to the empty-tabs UI.
        console.warn('[project-view] listProjectViews threw:', err)
      })
    return () => {
      cancelled = true
    }
  }, [activeProject, viewListByProject, settings])

  const handleSwitchView = useCallback(
    async (viewId: string) => {
      if (!activeProject) {
        return
      }
      const projectKey = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
      const current = lastViewByProject[projectKey]?.viewId
      if (current === viewId) {
        return
      }
      // Persist the new view selection so reloads & the picker stay in sync.
      // Why: read the freshest settings via getState() rather than the closure-
      // captured `settings` — between callback creation and invocation another
      // mutation (pin/recent update from elsewhere) may have landed, and the
      // closure value would clobber it on write.
      const freshSettings = useAppStore.getState().settings
      const prevSettings = freshSettings?.githubProjects ?? {
        pinned: [],
        recent: [],
        lastViewByProject: {},
        activeProject: null
      }
      await useAppStore.getState().updateSettings({
        githubProjects: {
          ...prevSettings,
          lastViewByProject: {
            ...prevSettings.lastViewByProject,
            [projectKey]: { viewId }
          }
        }
      })
      await doFetch({
        owner: activeProject.owner,
        ownerType: activeProject.ownerType,
        projectNumber: activeProject.number,
        viewId
      })
    },
    [activeProject, doFetch, lastViewByProject]
  )

  const currentProjectViewKey = useMemo(() => {
    if (!activeProject) {
      return null
    }
    const key = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
    const viewId = lastViewByProject[key]?.viewId
    if (!viewId) {
      return null
    }
    return `${key}:${viewId}`
  }, [activeProject, lastViewByProject])

  const currentAppliedOverride = currentProjectViewKey
    ? appliedQueryByView[currentProjectViewKey]
    : undefined

  const currentCacheKey = useMemo(() => {
    if (!activeProject) {
      return null
    }
    const key = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
    const viewId = lastViewByProject[key]?.viewId
    if (!viewId) {
      return null
    }
    return projectViewCacheKey(
      activeProject.ownerType,
      activeProject.owner,
      activeProject.number,
      viewId,
      currentAppliedOverride
    )
  }, [activeProject, lastViewByProject, currentAppliedOverride])

  const table: GitHubProjectTable | null = currentCacheKey
    ? (projectViewCache[currentCacheKey]?.data ?? null)
    : null
  // Why: render every project item, like GitHub itself. Rows whose repo isn't
  // added to Agentum are still handled per-row (the slug dialog / "Repository
  // not in Agentum" modal below), so filtering them out only made the panel
  // look empty — "No items match this view's filter" — for any project whose
  // repos aren't in Agentum (#11). `slugIndexReady`/`lookupSlug` stay in use to
  // route those per-row actions.
  const visibleTable = table

  // Parent-dropped toast, once per table.
  useEffect(() => {
    if (!table || !currentCacheKey || !table.parentFieldDropped) {
      return
    }
    if (parentDroppedToasted.has(currentCacheKey)) {
      return
    }
    toast.message('Sub-issue data is unavailable for your token.')
    setParentDroppedToasted((prev) => {
      const next = new Set(prev)
      next.add(currentCacheKey)
      return next
    })
  }, [table, currentCacheKey, parentDroppedToasted])

  const selectedViewUrl = table
    ? `${table.project.url}/views/${table.selectedView.number ?? ''}`
    : null

  // ── Row action state ────────────────────────────────────────────────
  // Why: when a row matches a registered repo, we open the full
  // `GitHubItemDialog` in repo-backed mode; when it doesn't, we open the
  // simplified slug-mode dialog. `repoNotInAgentum` drives the fallback modal
  // from the design doc's `repo-not-in-agentum` interaction state.
  const [dialogRepoItem, setDialogRepoItem] = useState<{
    workItem: GitHubWorkItem
    repoPath: string
    repoId: string
    origin: GitHubItemDialogProjectOrigin
  } | null>(null)
  // Why: the slug dialog is only opened for rows whose repo isn't registered
  // in Agentum (matched repos go through the full GitHubItemDialog above), so
  // there's no `matchedRepo` to track here. The repo-not-in-agentum modal —
  // owned by this parent, not the slug dialog — handles "Start work".
  const [slugDialog, setSlugDialog] = useState<{
    origin: GitHubItemDialogProjectOrigin
  } | null>(null)
  const [repoNotInAgentum, setRepoNotInAgentum] = useState<{
    owner: string
    repo: string
    url: string | null
  } | null>(null)
  const liveRepoIds = useMemo(() => new Set(repos.map((repo) => repo.id)), [repos])

  const resolvedDialogRepoItem = resolveRepoBackedProjectDialogState(dialogRepoItem, liveRepoIds)
  if (resolvedDialogRepoItem !== dialogRepoItem) {
    // Why: repo-backed Project dialogs cannot edit after their repo leaves
    // Agentum; clear them before the modal tree receives stale repo ids.
    setDialogRepoItem(resolvedDialogRepoItem)
  }

  const resolvedMissingRepoDialogs = resolveMissingRepoProjectDialogState({
    slugIndexReady,
    slugDialog,
    repoNotInAgentum,
    lookupSlug
  })
  if (resolvedMissingRepoDialogs.slugDialog !== slugDialog) {
    // Why: once a previously missing repo is registered, Project rows should
    // use the full repo-backed dialog instead of the slug fallback.
    setSlugDialog(resolvedMissingRepoDialogs.slugDialog)
  }
  if (resolvedMissingRepoDialogs.repoNotInAgentum !== repoNotInAgentum) {
    setRepoNotInAgentum(resolvedMissingRepoDialogs.repoNotInAgentum)
  }

  const buildWorkItem = useCallback(
    (row: GitHubProjectRow, repoId: string): GitHubWorkItem | null => {
      if (row.itemType !== 'ISSUE' && row.itemType !== 'PULL_REQUEST') {
        return null
      }
      if (row.content.number == null || !row.content.url) {
        return null
      }
      return {
        id: `${row.itemType === 'PULL_REQUEST' ? 'pr' : 'issue'}:${row.content.number}`,
        type: row.itemType === 'PULL_REQUEST' ? 'pr' : 'issue',
        number: row.content.number,
        title: row.content.title,
        state:
          row.content.state === 'MERGED'
            ? 'merged'
            : row.content.state === 'CLOSED'
              ? 'closed'
              : row.content.isDraft
                ? 'draft'
                : 'open',
        url: row.content.url,
        labels: row.content.labels.map((l) => l.name),
        updatedAt: row.updatedAt,
        author: null,
        repoId
      }
    },
    []
  )

  const buildOrigin = useCallback(
    (
      row: GitHubProjectRow,
      cacheKey: string,
      table: GitHubProjectTable
    ): GitHubItemDialogProjectOrigin | null => {
      if (row.itemType !== 'ISSUE' && row.itemType !== 'PULL_REQUEST') {
        return null
      }
      if (row.content.number == null || !row.content.repository) {
        return null
      }
      const [owner, repo] = row.content.repository.split('/')
      if (!owner || !repo) {
        return null
      }
      return {
        owner,
        repo,
        number: row.content.number,
        type: row.itemType === 'PULL_REQUEST' ? 'pr' : 'issue',
        projectId: table.project.id,
        projectItemId: row.id,
        cacheKey
      }
    },
    []
  )

  // Spec 015 F2: hop a multi-host work item into the new-workspace composer
  // seeded on a host that actually holds the repo. Mirrors TaskPage's
  // `openComposerForItem`: snapshot the issue body into `linkedContext` so the
  // spawned agent gets the spec, not just the URL — best-effort, a fetch
  // failure must never block the hop. Deliberately NO `startGatedRun` (board
  // Start-work is an ungated direct launch) and NO `initialBaseBranch` (the
  // wizard owns its own PR-head resolution).
  const openComposerForChoice = useCallback(
    async (
      item: GitHubWorkItem,
      match: { repos: Repo[]; seedRepoId: string },
      origin: { owner: string; repo: string }
    ): Promise<void> => {
      let linkedWorkItem: LinkedWorkItemSummary = {
        type: item.type,
        number: item.number,
        title: item.title,
        url: item.url
      }
      const seedRepo = match.repos.find((repo) => repo.id === match.seedRepoId)
      // Why: the body fetch runs `gh` against a local checkout, so only a
      // local seed's path is a usable workdir; a remote-only match set keeps
      // the title+URL fallback.
      const workdir = seedRepo && seedRepo.connectionId == null ? seedRepo.path : null
      if (item.type === 'issue' && workdir) {
        try {
          const fetched = await fetchGithubIssueBody({
            number: item.number,
            workdir,
            slug: `${origin.owner}/${origin.repo}`
          })
          linkedWorkItem = buildGithubIssueLinkedWorkItem(item, fetched)
        } catch (err) {
          // Best-effort: keep the title+URL fallback so the composer still opens.
          console.warn('[project-view] could not load GitHub issue body for Start work:', err)
        }
      }
      useAppStore.getState().openModal('new-workspace-composer', {
        linkedWorkItem,
        prefilledName: getLinkedWorkItemSuggestedName(item),
        initialRepoId: match.seedRepoId,
        telemetrySource: 'sidebar'
      })
    },
    []
  )

  // Spec 015 F2: the single classification point for both board start
  // gestures (row "Start work" and the item dialog's "Use") — a repo
  // registered on several hosts asks where via the wizard instead of
  // silently assuming local or falsely claiming it isn't in Agentum.
  const startWorkForItem = useCallback(
    (args: {
      /** Build the launchable item for the classified repo id; null aborts
       *  (redacted/draft rows can't launch). */
      buildItem: (repoId: string) => GitHubWorkItem | null
      origin: { owner: string; repo: string }
      url: string | null
    }): void => {
      const { buildItem, origin, url } = args
      const match = classifyStartWorkRepoMatches(lookupSlug(`${origin.owner}/${origin.repo}`))
      if (match.kind === 'none') {
        setRepoNotInAgentum({ owner: origin.owner, repo: origin.repo, url })
        return
      }
      if (match.kind === 'direct') {
        const workItem = buildItem(match.repo.id)
        if (!workItem) {
          return
        }
        void launchWorkItemDirect({
          item: workItem,
          repoId: match.repo.id,
          launchSource: 'task_page',
          telemetrySource: 'sidebar',
          openModalFallback: () => {
            // Why: when `launchWorkItemDirect` wants user input
            // (setupRunPolicy:'ask' or agent detection fails), fall back to
            // opening the URL so the user keeps a path forward. The composer
            // modal is app-mounted and reachable (the multi-host arm below
            // hops to it), but the single-match gesture stays byte-equivalent
            // to the pre-015 direct launch (AC 6).
            if (url) {
              void api.shell.openUrl(url)
            }
          }
        })
        return
      }
      const workItem = buildItem(match.seedRepoId)
      if (!workItem) {
        return
      }
      void openComposerForChoice(workItem, match, origin)
    },
    [lookupSlug, openComposerForChoice]
  )

  const handleOpenDialog = useCallback(
    (row: GitHubProjectRow) => {
      if (!currentCacheKey || !table) {
        return
      }
      const origin = buildOrigin(row, currentCacheKey, table)
      if (!origin) {
        // Redacted / draft / missing slug — fall back to opening GitHub.
        if (row.content.url) {
          void api.shell.openUrl(row.content.url)
        }
        return
      }
      const match = classifyStartWorkRepoMatches(lookupSlug(`${origin.owner}/${origin.repo}`))
      if (match.kind !== 'none') {
        // Why: a repo registered on several hosts still gets the full
        // repo-backed dialog — its mutations are slug-addressed (see the
        // dialog comment below), so any same-slug candidate is safe; seed
        // with the local copy when present (spec 015 F2).
        const repo =
          match.kind === 'direct'
            ? match.repo
            : (match.repos.find((candidate) => candidate.id === match.seedRepoId) ??
              match.repos[0])
        const workItem = buildWorkItem(row, repo.id)
        if (workItem) {
          setDialogRepoItem({ workItem, repoPath: repo.path, repoId: repo.id, origin })
          return
        }
      }
      // Unknown repo — use the simplified slug-mode dialog.
      setSlugDialog({ origin })
    },
    [currentCacheKey, table, buildOrigin, lookupSlug, buildWorkItem]
  )

  const handleStartWork = useCallback(
    (row: GitHubProjectRow) => {
      if (!currentCacheKey || !table) {
        return
      }
      const origin = buildOrigin(row, currentCacheKey, table)
      if (!origin) {
        return
      }
      startWorkForItem({
        buildItem: (repoId) => buildWorkItem(row, repoId),
        origin,
        url: row.content.url ?? null
      })
    },
    [currentCacheKey, table, buildOrigin, buildWorkItem, startWorkForItem]
  )

  const handleEditAssignees = useCallback(
    async (row: GitHubProjectRow, add: string[], remove: string[]) => {
      if (!currentCacheKey) {
        return
      }
      const res = await patchProjectIssueOrPr(currentCacheKey, row.id, {
        ...(add.length ? { addAssignees: add } : {}),
        ...(remove.length ? { removeAssignees: remove } : {})
      })
      if (!res.ok) {
        toast.error(res.error.message)
      }
    },
    [currentCacheKey, patchProjectIssueOrPr]
  )

  const handleEditLabels = useCallback(
    async (row: GitHubProjectRow, add: string[], remove: string[]) => {
      if (!currentCacheKey) {
        return
      }
      const res = await patchProjectIssueOrPr(currentCacheKey, row.id, {
        ...(add.length ? { addLabels: add } : {}),
        ...(remove.length ? { removeLabels: remove } : {})
      })
      if (!res.ok) {
        // `|| fallback`: a malformed error envelope must never blank the toast.
        toast.error(res.error?.message || 'Failed to update labels.')
      }
    },
    [currentCacheKey, patchProjectIssueOrPr]
  )

  const handleEditIssueType = useCallback(
    async (row: GitHubProjectRow, issueType: GitHubIssueType | null) => {
      if (!currentCacheKey) {
        return
      }
      const res = await patchProjectRowIssueType(currentCacheKey, row.id, issueType)
      if (!res.ok) {
        toast.error(res.error?.message || 'Failed to update the issue type.')
      }
    },
    [currentCacheKey, patchProjectRowIssueType]
  )

  const handleEditField = useCallback(
    async (
      row: GitHubProjectRow,
      fieldId: string,
      value: GitHubProjectFieldMutationValue | null
    ) => {
      if (!currentCacheKey) {
        return
      }
      const result =
        value === null
          ? await clearProjectFieldValue(currentCacheKey, row.id, fieldId)
          : await updateProjectFieldValue(currentCacheKey, row.id, fieldId, value)
      if (!result.ok) {
        toast.error(result.error?.message || 'Failed to update the field.')
      }
    },
    [clearProjectFieldValue, currentCacheKey, updateProjectFieldValue]
  )

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex min-w-0 flex-none flex-wrap items-center gap-2 border-b border-border/50 bg-muted/30 px-3 py-2">
        <ProjectPicker
          activeProject={
            activeProject && table
              ? {
                  owner: activeProject.owner,
                  ownerType: activeProject.ownerType,
                  number: activeProject.number,
                  title: table.project.title
                }
              : activeProject
                ? {
                    owner: activeProject.owner,
                    ownerType: activeProject.ownerType,
                    number: activeProject.number
                  }
                : null
          }
          onSelect={handleSelect}
        />
        {currentProjectViewKey ? (
          // Why: render the search input whenever a view is selected — even
          // while a refetch is in flight and `table` has briefly cleared for
          // the new cache key. Hiding the search box mid-search would make
          // it look like the search vanished. `key` keeps the local input
          // state stable across (project, view) changes only.
          <ProjectSearchInput
            key={currentProjectViewKey}
            viewFilter={table?.selectedView.filter ?? ''}
            appliedOverride={appliedQueryByView[currentProjectViewKey]}
            onApply={(nextOverride) => {
              if (!activeProject) {
                return
              }
              const key = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
              const viewId = lastViewByProject[key]?.viewId
              if (!viewId) {
                return
              }
              setAppliedQueryByView((prev) => {
                const next = { ...prev }
                if (nextOverride === undefined) {
                  delete next[currentProjectViewKey]
                } else {
                  next[currentProjectViewKey] = nextOverride
                }
                return next
              })
              // Why: force-fetch on user-initiated apply so the same
              // query re-typed (or cache-stale entries within TTL) does
              // not silently no-op.
              void doFetch(
                {
                  owner: activeProject.owner,
                  ownerType: activeProject.ownerType,
                  projectNumber: activeProject.number,
                  viewId
                },
                true,
                nextOverride
              )
            }}
          />
        ) : null}
        {table ? (
          <>
            <span className="ml-auto rounded-full border border-border/50 bg-background px-2 py-0.5 text-[11px]">
              {visibleTable?.totalCount ?? table.totalCount}
            </span>
            {selectedViewUrl ? (
              <Button
                variant="outline"
                size="icon"
                className="h-7 w-7"
                onClick={() => void api.shell.openUrl(selectedViewUrl)}
                aria-label="Open view in GitHub"
              >
                <ExternalLink className="size-3.5" />
              </Button>
            ) : null}
            <Button
              variant="outline"
              size="icon"
              className="h-7 w-7 cursor-pointer disabled:pointer-events-auto disabled:cursor-wait"
              onClick={() => {
                if (!activeProject || !currentCacheKey) {
                  return
                }
                const key = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
                const viewId = lastViewByProject[key]?.viewId
                if (!viewId) {
                  return
                }
                void doFetch(
                  {
                    owner: activeProject.owner,
                    ownerType: activeProject.ownerType,
                    projectNumber: activeProject.number,
                    viewId
                  },
                  true,
                  currentAppliedOverride
                )
              }}
              disabled={loading}
              aria-busy={loading}
              aria-label={loading ? 'Refreshing' : 'Refresh'}
              title={loading ? 'Refreshing' : 'Refresh'}
            >
              <RefreshCw className={cn('size-3.5', loading && 'animate-spin')} />
            </Button>
          </>
        ) : null}
      </div>

      {activeProject
        ? (() => {
            const projectKey = `${activeProject.ownerType}:${activeProject.owner}:${activeProject.number}`
            const views = viewListByProject[projectKey] ?? []
            const activeViewId = lastViewByProject[projectKey]?.viewId ?? null
            return (
              <ViewTabStrip
                views={views}
                activeViewId={activeViewId}
                onPick={(viewId) => void handleSwitchView(viewId)}
              />
            )
          })()
        : null}

      {!activeProject ? (
        <div className="flex flex-1 items-center justify-center p-8 text-sm text-muted-foreground">
          Choose a project to get started.
        </div>
      ) : loading && !table ? (
        <ProjectTableSkeleton />
      ) : error ? (
        <ErrorState
          error={error.error}
          totalCount={error.totalCount}
          onOpenInGitHub={() => {
            if (selectedViewUrl) {
              void api.shell.openUrl(selectedViewUrl)
            }
          }}
        />
      ) : visibleTable ? (
        visibleTable.selectedView.layout === 'BOARD_LAYOUT' ? (
          <ProjectBoardView
            table={visibleTable}
            onOpenDialog={handleOpenDialog}
            onEditField={handleEditField}
            onOpenInBrowser={(row) => {
              if (row.content.url) {
                void api.shell.openUrl(row.content.url)
              }
            }}
            onStartWork={handleStartWork}
          />
        ) : (
          <ProjectViewList
            table={visibleTable}
            onOpenDialog={handleOpenDialog}
            onEditField={handleEditField}
            onEditAssignees={(row, add, remove) => void handleEditAssignees(row, add, remove)}
            onEditLabels={(row, add, remove) => void handleEditLabels(row, add, remove)}
            onEditIssueType={(row, issueType) => void handleEditIssueType(row, issueType)}
            onOpenInBrowser={(row) => {
              if (row.content.url) {
                void api.shell.openUrl(row.content.url)
              }
            }}
            onStartWork={handleStartWork}
          />
        )
      ) : null}

      {/* Full repo-backed dialog — writes still go through slug-addressed
          mutation helpers (see design §Dialog editing from Project rows, line
          707) so a row from another repo cannot accidentally edit the active
          workspace. */}
      <GitHubItemDialog
        workItem={resolvedDialogRepoItem?.workItem ?? null}
        repoPath={resolvedDialogRepoItem?.repoPath ?? null}
        repoId={resolvedDialogRepoItem?.repoId ?? null}
        projectOrigin={resolvedDialogRepoItem?.origin}
        onUse={(item) => {
          const current = resolvedDialogRepoItem
          setDialogRepoItem(null)
          if (!current) {
            return
          }
          // Spec 015 F2: "Use" re-classifies against the slug index so a
          // multi-host repo routes through the same wizard hop as the row's
          // Start work — the dialog's repoId was only the seed candidate.
          startWorkForItem({
            buildItem: (repoId) => ({ ...item, repoId }),
            origin: { owner: current.origin.owner, repo: current.origin.repo },
            url: item.url ?? null
          })
        }}
        onClose={() => setDialogRepoItem(null)}
      />

      {/* Slug-only simplified dialog for rows whose repo isn't added to Agentum.
          Why: no Start-work affordance lives inside the slug dialog — the
          parent's `handleStartWork`/`repoNotInAgentum` modal owns that flow, so
          having a duplicate (always-disabled or always-routing-to-fallback)
          button here would only confuse the user. */}
      <ProjectItemSlugDialog
        projectOrigin={resolvedMissingRepoDialogs.slugDialog?.origin ?? null}
        onClose={() => setSlugDialog(null)}
      />

      {/* repo-not-in-agentum prompt: see design doc Interaction States. */}
      <Dialog
        open={resolvedMissingRepoDialogs.repoNotInAgentum !== null}
        onOpenChange={(open) => !open && setRepoNotInAgentum(null)}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Repository not in Agentum</DialogTitle>
            <DialogDescription>
              {resolvedMissingRepoDialogs.repoNotInAgentum
                ? `${resolvedMissingRepoDialogs.repoNotInAgentum.owner}/${resolvedMissingRepoDialogs.repoNotInAgentum.repo} isn't added to Agentum. Add it to start work, or open in GitHub.`
                : null}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2 sm:justify-end">
            <Button variant="ghost" onClick={() => setRepoNotInAgentum(null)}>
              Cancel
            </Button>
            {resolvedMissingRepoDialogs.repoNotInAgentum?.url ? (
              <Button
                variant="outline"
                onClick={() => {
                  if (resolvedMissingRepoDialogs.repoNotInAgentum?.url) {
                    void api.shell.openUrl(resolvedMissingRepoDialogs.repoNotInAgentum.url)
                  }
                  setRepoNotInAgentum(null)
                }}
              >
                Open in GitHub
              </Button>
            ) : null}
            <Button
              onClick={async () => {
                // Why: `addRepo` opens the OS folder picker — it's the only
                // non-destructive way to register a repo today. Auto-cloning
                // from a row click is out of v1 scope (design doc §Row
                // actions). Close the modal regardless so the user isn't
                // trapped if they cancel the picker.
                setRepoNotInAgentum(null)
                await addRepoFromStore()
              }}
            >
              Add repo
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

// Why: owns the transient search input string locally so typing does not
// re-render the parent (and therefore not the table). The parent only learns
// the value when the user applies it (Enter/blur/clear), which is the only
// moment that should trigger a refetch. Pre-populated from the view's stored
// filter and remounted (via `key`) when the active project/view changes.
function ProjectSearchInput({
  viewFilter,
  appliedOverride,
  onApply
}: {
  viewFilter: string
  appliedOverride: string | undefined
  onApply: (nextOverride: string | undefined) => void
}): React.JSX.Element {
  const initial = appliedOverride !== undefined ? appliedOverride : viewFilter
  const [value, setValue] = useState<string>(initial)
  const inputRef = useRef<HTMLInputElement>(null)
  const applied = appliedOverride !== undefined ? appliedOverride : viewFilter
  const dirty = value !== applied

  const apply = (next: string): void => {
    // Why: when the user reverts to the view's stored filter, drop the
    // override so the cache key collapses back onto the unfiltered entry.
    onApply(next === viewFilter ? undefined : next)
  }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      const isMac = navigator.userAgent.includes('Mac')
      const modifierPressed = isMac ? event.metaKey : event.ctrlKey
      if (!modifierPressed || event.altKey || event.shiftKey || event.key.toLowerCase() !== 'f') {
        return
      }
      if (document.querySelector('[role="dialog"]')) {
        return
      }

      const input = inputRef.current
      if (!input) {
        return
      }
      const target = event.target
      if (
        target instanceof HTMLElement &&
        target !== input &&
        (target instanceof HTMLInputElement ||
          target instanceof HTMLTextAreaElement ||
          target.isContentEditable)
      ) {
        return
      }

      event.preventDefault()
      event.stopPropagation()
      input.focus()
      input.select()
    }

    window.addEventListener('keydown', onKeyDown, { capture: true })
    return () => window.removeEventListener('keydown', onKeyDown, { capture: true })
  }, [])

  return (
    <div className="relative min-w-0 max-w-xl flex-1 basis-64">
      <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
      <Input
        ref={inputRef}
        data-github-project-search-input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            if (e.nativeEvent.isComposing) {
              return
            }
            e.preventDefault()
            apply(value)
          } else if (e.key === 'Escape') {
            setValue(applied)
            ;(e.target as HTMLInputElement).blur()
          }
        }}
        onBlur={() => {
          if (dirty) {
            apply(value)
          }
        }}
        placeholder={viewFilter || 'GitHub search, e.g. assignee:@me is:open'}
        title={viewFilter ? `View filter: ${viewFilter}` : undefined}
        className={cn(
          'h-7 rounded-md border-border/50 bg-background pl-8 pr-7 text-[11px]',
          dirty && 'border-amber-500/50'
        )}
      />
      {value ? (
        <button
          type="button"
          aria-label="Clear search"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            setValue('')
            apply('')
          }}
          className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground transition hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      ) : null}
    </div>
  )
}

function ViewTabStrip({
  views,
  activeViewId,
  onPick
}: {
  views: GitHubProjectViewSummary[]
  activeViewId: string | null
  onPick: (viewId: string) => void
}): React.JSX.Element {
  // Why: emulate GitHub Projects' tab strip — pill-shaped active tab with
  // layout icon, sitting on a muted base bar with a bottom border. Inactive
  // tabs are flat text; active gets a card background + outline. Disabled
  // (non-table) layouts stay visible at low opacity.
  return (
    <div className="project-view-tab-strip flex min-h-[41px] min-w-0 flex-none items-end gap-1 overflow-x-auto overflow-y-hidden border-b border-border/50 bg-muted/20 px-3 pt-3">
      {views.map((v) => {
        // Table and Board (Kanban) layouts both render in-app; Roadmap does not.
        const supported = v.layout === 'TABLE_LAYOUT' || v.layout === 'BOARD_LAYOUT'
        const active = v.id === activeViewId
        const layoutLabel =
          v.layout === 'BOARD_LAYOUT'
            ? 'Board'
            : v.layout === 'ROADMAP_LAYOUT'
              ? 'Roadmap'
              : 'Table'
        const Icon =
          v.layout === 'BOARD_LAYOUT'
            ? KanbanSquare
            : v.layout === 'ROADMAP_LAYOUT'
              ? MapIcon
              : TableIcon
        const tab = (
          <button
            key={v.id}
            type="button"
            disabled={!supported}
            onClick={() => onPick(v.id)}
            title={
              supported
                ? v.name
                : `${v.name} — Agentum doesn't support ${layoutLabel} project views yet. File a feature request at ${AGENTUM_FEATURE_REQUEST_URL}.`
            }
            className={cn(
              'inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-t-md border-x border-t px-3 py-1.5 text-xs',
              active
                ? '-mb-px border-border/60 bg-background text-foreground'
                : 'border-transparent text-muted-foreground hover:bg-background/40 hover:text-foreground',
              !supported &&
                'pointer-events-none cursor-not-allowed opacity-50 hover:bg-transparent hover:text-muted-foreground'
            )}
          >
            <Icon className="size-3.5 shrink-0 text-muted-foreground" />
            <span className={cn(active && 'font-medium')}>{v.name}</span>
          </button>
        )
        if (supported) {
          return tab
        }
        const unsupportedMessage = `Agentum doesn't support ${layoutLabel} project views yet.`
        return (
          <HoverCard key={v.id} openDelay={200} closeDelay={100}>
            <HoverCardTrigger asChild>
              <span
                tabIndex={0}
                aria-label={`${v.name}. ${unsupportedMessage} File a feature request at ${AGENTUM_FEATURE_REQUEST_URL}.`}
                className="inline-flex shrink-0 cursor-not-allowed rounded-t-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
              >
                {tab}
              </span>
            </HoverCardTrigger>
            <HoverCardContent side="bottom" align="start" sideOffset={8} className="w-72 p-3">
              <div className="space-y-2">
                <p className="text-xs leading-5 text-muted-foreground">
                  {unsupportedMessage} Switch to a Table view to work with this project in Agentum.
                </p>
                <Button
                  type="button"
                  size="xs"
                  variant="outline"
                  onClick={() => void api.shell.openUrl(AGENTUM_FEATURE_REQUEST_URL)}
                >
                  File feature request
                  <ExternalLink className="size-3" />
                </Button>
              </div>
            </HoverCardContent>
          </HoverCard>
        )
      })}
    </div>
  )
}

function ErrorState({
  error,
  totalCount,
  onOpenInGitHub
}: {
  error: GitHubProjectViewError
  totalCount?: number
  onOpenInGitHub: () => void
}): React.JSX.Element {
  // Auth/scope errors get a richer remediation UI driven by `gh auth
  // status`. Bail early so the generic `command`/`copy` block below is
  // only computed for non-auth error types.
  if (error.type === 'auth_required' || error.type === 'scope_missing') {
    return (
      <div className="flex flex-1 flex-col items-start gap-3 p-6 text-sm">
        <GhAuthErrorHelp
          error={error as GitHubProjectViewError & { type: 'auth_required' | 'scope_missing' }}
        />
        <Button size="sm" variant="outline" onClick={onOpenInGitHub}>
          <ExternalLink className="mr-1 size-3.5" /> Open in GitHub
        </Button>
      </div>
    )
  }
  const copy =
    error.type === 'too_large'
      ? `This view has ${totalCount ?? 'many'} items — too large to render in Agentum. Narrow the view's filter on GitHub.`
      : error.type === 'unsupported_layout'
        ? 'Agentum only renders table views yet. This is a Board or Roadmap view.'
        : error.type === 'not_found'
          ? 'Could not find this project or view.'
          : error.type === 'schema_drift'
            ? 'Could not read this project view.'
            : error.message
  return (
    <div className="flex flex-1 flex-col items-start gap-3 p-6 text-sm">
      <div className="text-muted-foreground">{copy}</div>
      <div className="flex gap-2">
        <Button size="sm" variant="outline" onClick={onOpenInGitHub}>
          <ExternalLink className="mr-1 size-3.5" /> Open in GitHub
        </Button>
      </div>
    </div>
  )
}

// Why: matches the shape of ProjectViewList's header + rows so the table
// doesn't visibly jump in height when real data lands. A 12-row stub fills
// a typical viewport at the table's min-h-10 row height.
function ProjectTableSkeleton(): React.JSX.Element {
  const headerCols = 6
  const bodyCols = 5
  return (
    <div
      aria-busy="true"
      aria-label="Loading project view"
      className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
    >
      <div className="grid items-center gap-3 border-b border-border/60 bg-background/95 px-3 py-2">
        <div
          className="grid items-center gap-3"
          style={{
            gridTemplateColumns: `repeat(${headerCols}, minmax(0, 1fr))`
          }}
        >
          {Array.from({ length: headerCols }).map((_, i) => (
            <div key={i} className="h-3 w-20 animate-pulse rounded bg-muted/70" />
          ))}
        </div>
      </div>
      <div className="divide-y divide-border/30">
        {Array.from({ length: 12 }).map((_, i) => (
          <div
            key={i}
            className="grid min-h-10 items-center gap-3 px-3 py-2"
            style={{ gridTemplateColumns: `repeat(${bodyCols}, minmax(0, 1fr))` }}
          >
            <div className="h-4 w-3/5 animate-pulse rounded bg-muted/70" />
            <div className="h-4 w-4/5 animate-pulse rounded bg-muted/70" />
            <div className="h-4 w-2/5 animate-pulse rounded-full bg-muted/60" />
            <div className="h-4 w-3/5 animate-pulse rounded bg-muted/60" />
            <div className="h-4 w-1/2 animate-pulse rounded bg-muted/60" />
          </div>
        ))}
      </div>
    </div>
  )
}
