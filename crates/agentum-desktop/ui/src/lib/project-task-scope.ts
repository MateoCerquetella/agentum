import type { Repo } from '@/shared/types'
import type { ProjectBindingDto } from '@/runtime/github-projects-client'
import type { LinearProjectDetail } from '@/shared/types'
import type {
  ProjectTrackerBoardBinding,
  ProjectTrackerGithubTarget,
  ProjectTrackerLinearTarget,
  ProjectTrackerProvider
} from '@/shared/project-tracker-config'

type ScopeBase = Readonly<{ repoId: string; repoName: string; generation: number }>
export type ProjectTaskScope =
  | (ScopeBase & { status: 'loading' })
  | (ScopeBase & { status: 'unbound'; provider: ProjectTrackerProvider | null; reason: 'provider-unset' | 'github-unbound' | 'linear-unbound' })
  | (ScopeBase & { status: 'unavailable'; provider: 'github' | 'linear'; reason: 'connection' | 'authorization' | 'not-found' | 'invalid-binding' | 'transport'; message: string })
  | (ScopeBase & { status: 'bound'; provider: 'github'; target: 'repository'; scopeKey: string; repoSlug: string })
  | (ScopeBase & { status: 'bound'; provider: 'github'; target: 'project'; scopeKey: string; repoSlug: string; projectId: string; owner: string; ownerType: 'user' | 'organization'; projectNumber: number; projectTitle: string })
  | (ScopeBase & { status: 'bound'; provider: 'linear'; scopeKey: string; workspaceId: string; workspaceName: string; projectId: string; projectName: string; projectUrl?: string; teamIds: readonly string[] })

export type GithubProjectTaskScope = Extract<
  ProjectTaskScope,
  { status: 'bound'; provider: 'github'; target: 'project' }
>
export type GithubRepositoryTaskScope = Extract<
  ProjectTaskScope,
  { status: 'bound'; provider: 'github'; target: 'repository' }
>

type GithubProjectBindingIdentity = ProjectBindingDto | ProjectTrackerBoardBinding

export function projectTaskScopeKey(input: { repoId: string } & ({ provider: 'github'; repoSlug: string; projectId: string } | { provider: 'linear'; workspaceId: string; projectId: string })): string {
  return JSON.stringify(input.provider === 'github' ? [input.repoId, 'github', input.repoSlug, input.projectId] : [input.repoId, 'linear', input.workspaceId, input.projectId])
}

export function loadingProjectTaskScope(repo: Repo, generation: number): ProjectTaskScope {
  return Object.freeze({ status: 'loading', repoId: repo.id, repoName: repo.displayName, generation })
}

export function unboundProjectTaskScope(
  repo: Repo,
  generation: number,
  canonicalProvider?: ProjectTrackerProvider | null
): ProjectTaskScope {
  const legacyProvider =
    repo.trackerProvider === 'github' || repo.trackerProvider === 'linear'
      ? repo.trackerProvider
      : null
  const provider = canonicalProvider === undefined ? legacyProvider : canonicalProvider
  const reason = provider === 'github' ? 'github-unbound' : provider === 'linear' ? 'linear-unbound' : 'provider-unset'
  return Object.freeze({ status: 'unbound', repoId: repo.id, repoName: repo.displayName, generation, provider, reason })
}

export function githubProjectTaskScope(repo: Repo, generation: number, slug: string, binding: GithubProjectBindingIdentity): ProjectTaskScope {
  if (!binding.projectId || !binding.projectOwner || (binding.projectOwnerType !== 'user' && binding.projectOwnerType !== 'organization') || !binding.projectNumber) {
    return Object.freeze({ status: 'unavailable', provider: 'github', reason: 'invalid-binding', message: 'The GitHub board binding is incomplete.', repoId: repo.id, repoName: repo.displayName, generation })
  }
  return Object.freeze({ status: 'bound', provider: 'github', target: 'project', repoId: repo.id, repoName: repo.displayName, generation, scopeKey: projectTaskScopeKey({ repoId: repo.id, provider: 'github', repoSlug: slug, projectId: binding.projectId }), repoSlug: slug, projectId: binding.projectId, owner: binding.projectOwner, ownerType: binding.projectOwnerType, projectNumber: binding.projectNumber, projectTitle: binding.projectTitle ?? `${binding.projectOwner}/#${binding.projectNumber}` })
}

export function githubTrackerTaskScope(
  repo: Repo,
  generation: number,
  target: ProjectTrackerGithubTarget
): ProjectTaskScope {
  const binding = target.projectBinding
  if (!binding) {
    return Object.freeze({
      status: 'bound',
      provider: 'github',
      target: 'repository',
      repoId: repo.id,
      repoName: repo.displayName,
      generation,
      scopeKey: JSON.stringify([repo.id, 'github', target.repositorySlug, 'repository']),
      repoSlug: target.repositorySlug
    })
  }
  return githubProjectTaskScope(repo, generation, target.repositorySlug, binding)
}

export function linearProjectTaskScope(
  repo: Repo,
  generation: number,
  project: LinearProjectDetail,
  canonicalTarget?: ProjectTrackerLinearTarget,
  canonicalWorkspaceName?: string
): ProjectTaskScope {
  const legacyBinding = repo.linearProjectBinding
  const workspaceId = canonicalTarget?.workspaceId ?? legacyBinding?.workspaceId
  const projectId =
    canonicalTarget?.scope?.kind === 'project'
      ? canonicalTarget.scope.id
      : legacyBinding?.projectId
  if (!workspaceId || !projectId || project.id !== projectId || project.workspaceId !== workspaceId) {
    return Object.freeze({ status: 'unavailable', provider: 'linear', reason: 'invalid-binding', message: 'The Linear project no longer matches this project binding.', repoId: repo.id, repoName: repo.displayName, generation })
  }
  const teamIds = (project.teams ?? []).map((team) => team.id)
  if (canonicalTarget?.teamId && !teamIds.includes(canonicalTarget.teamId)) {
    teamIds.unshift(canonicalTarget.teamId)
  }
  return Object.freeze({ status: 'bound', provider: 'linear', repoId: repo.id, repoName: repo.displayName, generation, scopeKey: projectTaskScopeKey({ repoId: repo.id, provider: 'linear', workspaceId, projectId }), workspaceId, workspaceName: canonicalWorkspaceName ?? project.workspaceName ?? legacyBinding?.workspaceName ?? workspaceId, projectId, projectName: project.name || legacyBinding?.projectName || projectId, ...(project.url || legacyBinding?.projectUrl ? { projectUrl: project.url ?? legacyBinding?.projectUrl } : {}), teamIds: Object.freeze(teamIds) })
}
