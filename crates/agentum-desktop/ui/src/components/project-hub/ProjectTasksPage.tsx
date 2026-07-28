import { useEffect, useRef, useState } from 'react'
import type { Repo } from '@/shared/types'
import type { ProjectTaskScope } from '@/lib/project-task-scope'
import {
  githubTrackerTaskScope,
  linearProjectTaskScope,
  loadingProjectTaskScope,
  unboundProjectTaskScope
} from '@/lib/project-task-scope'
import { linearGetProject, linearStatus } from '@/runtime/runtime-linear-client'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { captureProjectTaskScopeGuard } from '@/lib/project-task-scope-guard'
import { publishProjectTaskScopeAuthority } from '@/lib/project-task-scope-authority'
import ProjectViewWrapper from '@/components/github-project/ProjectViewWrapper'
import { PROJECT_INTEGRATIONS_SECTION_ID } from '@/components/settings/ProjectIntegrationsSection'
import { LockedLinearProjectTasks } from './LockedLinearProjectTasks'
import { LockedGithubRepoTasks } from './LockedGithubRepoTasks'
import { TrackerIntakePanel } from './TrackerIntakePanel'

export function ProjectTasksPage({ repo }: { repo: Repo }): React.JSX.Element {
  const settings = useAppStore((state) => state.settings)
  const config = useAppStore((state) => state.projectTrackerConfigByRepo[repo.id])
  const configLoadStatus = useAppStore(
    (state) => state.projectTrackerLoadStatusByRepo[repo.id] ?? 'idle'
  )
  const configError = useAppStore((state) => state.projectTrackerErrorByRepo[repo.id])
  const loadConfig = useAppStore((state) => state.loadProjectTrackerConfig)
  const generationRef = useRef(0)
  const [scope, setScope] = useState<ProjectTaskScope>(() => loadingProjectTaskScope(repo, 0))

  useEffect(() => {
    void loadConfig(repo.id).catch(() => undefined)
  }, [loadConfig, repo.id])

  useEffect(() => {
    const generation = ++generationRef.current
    setScope(loadingProjectTaskScope(repo, generation))
    let cancelled = false
    let revokeAuthority: (() => void) | null = null
    const publish = (next: ProjectTaskScope): void => {
      if (cancelled || generationRef.current !== generation || repo.id !== next.repoId) return

      // Descendant passive effects run before their parent's. Install the
      // immutable repo/provider authority before rendering guarded children.
      revokeAuthority?.()
      const guard = captureProjectTaskScopeGuard(next)
      revokeAuthority = guard ? publishProjectTaskScopeAuthority(guard) : null
      setScope(next)
    }

    if (configLoadStatus !== 'loaded') {
      return () => {
        cancelled = true
        revokeAuthority?.()
      }
    }
    if (!config?.provider) {
      publish(unboundProjectTaskScope(repo, generation, null))
      return () => {
        cancelled = true
        revokeAuthority?.()
      }
    }
    if (config.provider === 'github') {
      if (!config.github) {
        publish({
          status: 'unavailable',
          provider: 'github',
          reason: 'invalid-binding',
          message: 'The canonical GitHub tracker target is incomplete.',
          repoId: repo.id,
          repoName: repo.displayName,
          generation
        })
      } else {
        publish(githubTrackerTaskScope(repo, generation, config.github))
      }
      return () => {
        cancelled = true
        revokeAuthority?.()
      }
    }

    const target = config.linear
    if (!target || target.scope?.kind !== 'project') {
      publish({
        status: 'unavailable',
        provider: 'linear',
        reason: 'invalid-binding',
        message: 'Choose a Linear Project in Project Settings.',
        repoId: repo.id,
        repoName: repo.displayName,
        generation
      })
      return () => {
        cancelled = true
        revokeAuthority?.()
      }
    }
    const projectScope = target.scope

    void (async () => {
      try {
        const status = await linearStatus(settings)
        if (cancelled || generationRef.current !== generation) return
        const workspace = (status.workspaces ?? []).find(
          (candidate) => candidate.id === target.workspaceId
        )
        if (!status.connected || !workspace) {
          publish({
            status: 'unavailable',
            provider: 'linear',
            reason: 'connection',
            message: `The configured Linear workspace ${target.workspaceId} is not connected.`,
            repoId: repo.id,
            repoName: repo.displayName,
            generation
          })
          return
        }
        const project = await linearGetProject(
          settings,
          projectScope.id,
          target.workspaceId
        )
        if (!project) {
          publish({
            status: 'unavailable',
            provider: 'linear',
            reason: 'not-found',
            message: `The configured Linear project ${projectScope.id} is unavailable.`,
            repoId: repo.id,
            repoName: repo.displayName,
            generation
          })
          return
        }
        publish(
          linearProjectTaskScope(
            repo,
            generation,
            project,
            target,
            workspace.organizationName
          )
        )
      } catch (cause) {
        publish({
          status: 'unavailable',
          provider: 'linear',
          reason: 'transport',
          message: cause instanceof Error ? cause.message : 'Could not load the Linear tracker.',
          repoId: repo.id,
          repoName: repo.displayName,
          generation
        })
      }
    })()

    return () => {
      cancelled = true
      revokeAuthority?.()
      generationRef.current += 1
    }
  }, [config, configLoadStatus, repo, settings])

  const openSettings = (): void => {
    const store = useAppStore.getState()
    store.openSettingsTarget({
      pane: 'repo',
      repoId: repo.id,
      sectionId: PROJECT_INTEGRATIONS_SECTION_ID
    })
    store.openSettingsPage()
  }

  const trackerName =
    scope.status === 'bound'
      ? scope.provider === 'linear'
        ? scope.projectName
        : scope.target === 'project'
          ? scope.projectTitle
          : scope.repoSlug
      : config?.provider
        ? config.provider === 'github'
          ? config.github?.repositorySlug ?? 'GitHub'
          : 'Linear'
        : 'Not configured'

  const trackerHeader = (
    <div className="flex flex-none flex-col gap-2 border-b border-border/70 px-4 py-3 sm:flex-row sm:items-center">
      <div className="min-w-0 flex-1">
        <p className="font-mono text-[10px] uppercase tracking-[0.14em] text-muted-foreground">
          Tracker
        </p>
        <p className="truncate text-sm font-medium">{trackerName}</p>
      </div>
      <Button size="sm" variant="outline" onClick={openSettings}>
        Project Settings
      </Button>
    </div>
  )

  if (configLoadStatus === 'error') {
    return (
      <div className="flex h-full min-h-0 flex-col">
        {trackerHeader}
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
          <p className="text-sm font-medium">Could not load {repo.displayName}&apos;s tracker.</p>
          <p className="max-w-lg text-xs text-muted-foreground">{configError}</p>
          <Button
            size="sm"
            onClick={() => void loadConfig(repo.id, { force: true }).catch(() => undefined)}
          >
            Retry
          </Button>
        </div>
      </div>
    )
  }

  if (scope.status === 'loading') {
    return (
      <div className="flex h-full min-h-0 flex-col">
        {trackerHeader}
        <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
          Loading work from {repo.displayName}&apos;s configured tracker…
        </div>
      </div>
    )
  }

  if (scope.status === 'unbound' || scope.status === 'unavailable') {
    return (
      <div className="flex h-full min-h-0 flex-col">
        {trackerHeader}
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
          <p className="text-sm font-medium">
            {scope.status === 'unavailable'
              ? `${repo.displayName}'s configured tracker is unavailable.`
              : `No tracker is configured for ${repo.displayName}.`}
          </p>
          <p className="max-w-lg text-xs text-muted-foreground">
            {scope.status === 'unavailable'
              ? scope.message
              : 'Choose a GitHub or Linear tracker in Project Settings.'}
          </p>
          <Button size="sm" onClick={openSettings}>
            Open Project Settings
          </Button>
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {trackerHeader}
      <details className="flex-none border-b border-border/60">
        <summary className="cursor-pointer px-4 py-2 text-xs font-medium">
          New issue · {scope.provider === 'github' ? 'GitHub' : 'Linear'}
        </summary>
        <div className="max-h-[55vh] overflow-auto border-t border-border/50 p-4">
          <TrackerIntakePanel repo={repo} scope={scope} />
        </div>
      </details>
      <div className="min-h-0 flex-1">
        {scope.provider === 'github' ? (
          scope.target === 'project' ? (
            <ProjectViewWrapper key={scope.scopeKey} repoId={repo.id} lockedScope={scope} />
          ) : (
            <LockedGithubRepoTasks key={scope.scopeKey} repo={repo} scope={scope} />
          )
        ) : (
          <LockedLinearProjectTasks key={scope.scopeKey} scope={scope} />
        )}
      </div>
    </div>
  )
}
