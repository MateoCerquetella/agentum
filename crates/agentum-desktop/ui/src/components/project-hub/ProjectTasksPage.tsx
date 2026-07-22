import { useEffect, useRef, useState } from 'react'
import type { Repo } from '@/shared/types'
import type { ProjectTaskScope } from '@/lib/project-task-scope'
import { githubProjectTaskScope, linearProjectTaskScope, loadingProjectTaskScope, unboundProjectTaskScope } from '@/lib/project-task-scope'
import { GithubProjectsBindingError, getProjectBinding } from '@/runtime/github-projects-client'
import { linearGetProject, linearStatus } from '@/runtime/runtime-linear-client'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { captureProjectTaskScopeGuard } from '@/lib/project-task-scope-guard'
import { publishProjectTaskScopeAuthority } from '@/lib/project-task-scope-authority'
import ProjectViewWrapper from '@/components/github-project/ProjectViewWrapper'
import { PROJECT_INTEGRATIONS_SECTION_ID } from '@/components/settings/ProjectIntegrationsSection'
import { LockedLinearProjectTasks } from './LockedLinearProjectTasks'
import { TrackerIntakePanel } from './TrackerIntakePanel'

export function ProjectTasksPage({ repo }: { repo: Repo }): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const generationRef = useRef(0)
  const [scope, setScope] = useState<ProjectTaskScope>(() => loadingProjectTaskScope(repo, 0))
  const scopeRef = useRef<ProjectTaskScope>(scope)
  scopeRef.current = scope
  useEffect(() => {
    const guard = captureProjectTaskScopeGuard(scope)
    return guard ? publishProjectTaskScopeAuthority(guard) : undefined
  }, [scope])

  useEffect(() => {
    const generation = ++generationRef.current
    const loading = loadingProjectTaskScope(repo, generation)
    scopeRef.current = loading
    setScope(loading)
    let cancelled = false
    const publish = (next: ProjectTaskScope): void => { if (!cancelled && generationRef.current === generation && repo.id === next.repoId) { scopeRef.current = next; setScope(next) } }
    if (repo.trackerProvider !== 'github' && repo.trackerProvider !== 'linear') { publish(unboundProjectTaskScope(repo, generation)); return () => { cancelled = true } }
    if (repo.trackerProvider === 'github') {
      void getProjectBinding({ workdir: repo.path, repoId: repo.id }).then((result) => publish(result.binding ? githubProjectTaskScope(repo, generation, result.slug, result.binding) : unboundProjectTaskScope(repo, generation))).catch((cause) => publish({ status: 'unavailable', provider: 'github', reason: cause instanceof GithubProjectsBindingError && cause.code === 'auth_required' ? 'authorization' : 'transport', message: cause instanceof Error ? cause.message : 'Could not load the GitHub binding.', repoId: repo.id, repoName: repo.displayName, generation }))
    } else if (!repo.linearProjectBinding) publish(unboundProjectTaskScope(repo, generation))
    else void (async () => {
      try {
        const binding = repo.linearProjectBinding
        const status = await linearStatus(settings)
        if (cancelled || generationRef.current !== generation) return
        if (!status.connected || !(status.workspaces ?? []).some((workspace) => workspace.id === binding.workspaceId)) { publish({ status: 'unavailable', provider: 'linear', reason: 'connection', message: `The Linear workspace ${binding.workspaceName} is not connected.`, repoId: repo.id, repoName: repo.displayName, generation }); return }
        const project = await linearGetProject(settings, binding.projectId, binding.workspaceId)
        if (!project) { publish({ status: 'unavailable', provider: 'linear', reason: 'not-found', message: `The Linear project ${binding.projectName} is unavailable.`, repoId: repo.id, repoName: repo.displayName, generation }); return }
        publish(linearProjectTaskScope(repo, generation, project))
      } catch (cause) { publish({ status: 'unavailable', provider: 'linear', reason: 'transport', message: cause instanceof Error ? cause.message : 'Could not load the Linear binding.', repoId: repo.id, repoName: repo.displayName, generation }) }
    })()
    return () => { cancelled = true; generationRef.current += 1 }
  }, [repo.id, repo.path, repo.displayName, repo.trackerProvider, repo.linearProjectBinding, settings])

  const openSettings = (): void => { const store = useAppStore.getState(); store.openSettingsTarget({ pane: 'repo', repoId: repo.id, sectionId: PROJECT_INTEGRATIONS_SECTION_ID }); store.openSettingsPage() }
  if (scope.status === 'loading') return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Loading work from {repo.displayName}&apos;s configured tracker…</div>
  if (scope.status === 'unbound' || scope.status === 'unavailable') return <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center"><p className="text-sm font-medium">{scope.status === 'unavailable' ? `${repo.displayName}'s configured tracker is unavailable.` : `No tracker is configured for ${repo.displayName}.`}</p>{scope.status === 'unavailable' ? <p className="max-w-lg text-xs text-muted-foreground">{scope.message}</p> : <p className="text-xs text-muted-foreground">Choose a GitHub or Linear tracker in Project Settings.</p>}<Button size="sm" onClick={openSettings}>Open Project Settings</Button></div>
  return <div className="flex h-full min-h-0 flex-col">
    <details className="flex-none border-b border-border/60">
      <summary className="cursor-pointer px-4 py-2 text-xs font-medium">{scope.provider === 'github' ? scope.projectTitle : scope.projectName} · New issue</summary>
      <div className="max-h-[55vh] overflow-auto border-t border-border/50 p-4">
        <div className="mb-3 flex items-center justify-between gap-3 text-xs text-muted-foreground"><span>Read-only binding summary. Change this tracker in Project Settings.</span><Button size="sm" variant="outline" onClick={openSettings}>Open Project Settings</Button></div>
        <TrackerIntakePanel repo={repo} scope={scope} />
      </div>
    </details>
    <div className="min-h-0 flex-1">{scope.provider === 'github' ? <ProjectViewWrapper key={scope.scopeKey} repoId={repo.id} lockedScope={scope} /> : <LockedLinearProjectTasks key={scope.scopeKey} scope={scope} />}</div>
  </div>
}
