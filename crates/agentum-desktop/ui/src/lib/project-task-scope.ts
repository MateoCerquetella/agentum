import type { Repo } from '@/shared/types'
import type { ProjectBindingDto } from '@/runtime/github-projects-client'
import type { LinearProjectDetail } from '@/shared/types'

type ScopeBase = Readonly<{ repoId: string; repoName: string; generation: number }>
export type ProjectTaskScope =
  | (ScopeBase & { status: 'loading' })
  | (ScopeBase & { status: 'unbound'; provider: 'github' | 'linear' | 'auto' | null; reason: 'provider-unset' | 'github-unbound' | 'linear-unbound' })
  | (ScopeBase & { status: 'unavailable'; provider: 'github' | 'linear'; reason: 'connection' | 'authorization' | 'not-found' | 'invalid-binding' | 'transport'; message: string })
  | (ScopeBase & { status: 'bound'; provider: 'github'; scopeKey: string; repoSlug: string; projectId: string; owner: string; ownerType: 'user' | 'organization'; projectNumber: number; projectTitle: string })
  | (ScopeBase & { status: 'bound'; provider: 'linear'; scopeKey: string; workspaceId: string; workspaceName: string; projectId: string; projectName: string; projectUrl?: string; teamIds: readonly string[] })

export function projectTaskScopeKey(input: { repoId: string } & ({ provider: 'github'; repoSlug: string; projectId: string } | { provider: 'linear'; workspaceId: string; projectId: string })): string {
  return JSON.stringify(input.provider === 'github' ? [input.repoId, 'github', input.repoSlug, input.projectId] : [input.repoId, 'linear', input.workspaceId, input.projectId])
}

export function loadingProjectTaskScope(repo: Repo, generation: number): ProjectTaskScope {
  return Object.freeze({ status: 'loading', repoId: repo.id, repoName: repo.displayName, generation })
}

export function unboundProjectTaskScope(repo: Repo, generation: number): ProjectTaskScope {
  const provider = repo.trackerProvider ?? null
  const reason = provider === 'github' ? 'github-unbound' : provider === 'linear' ? 'linear-unbound' : 'provider-unset'
  return Object.freeze({ status: 'unbound', repoId: repo.id, repoName: repo.displayName, generation, provider, reason })
}

export function githubProjectTaskScope(repo: Repo, generation: number, slug: string, binding: ProjectBindingDto): ProjectTaskScope {
  if (!binding.projectId || !binding.projectOwner || (binding.projectOwnerType !== 'user' && binding.projectOwnerType !== 'organization') || !binding.projectNumber) {
    return Object.freeze({ status: 'unavailable', provider: 'github', reason: 'invalid-binding', message: 'The GitHub board binding is incomplete.', repoId: repo.id, repoName: repo.displayName, generation })
  }
  return Object.freeze({ status: 'bound', provider: 'github', repoId: repo.id, repoName: repo.displayName, generation, scopeKey: projectTaskScopeKey({ repoId: repo.id, provider: 'github', repoSlug: slug, projectId: binding.projectId }), repoSlug: slug, projectId: binding.projectId, owner: binding.projectOwner, ownerType: binding.projectOwnerType, projectNumber: binding.projectNumber, projectTitle: binding.projectTitle ?? `${binding.projectOwner}/#${binding.projectNumber}` })
}

export function linearProjectTaskScope(repo: Repo, generation: number, project: LinearProjectDetail): ProjectTaskScope {
  const binding = repo.linearProjectBinding
  if (!binding || project.id !== binding.projectId || project.workspaceId !== binding.workspaceId) {
    return Object.freeze({ status: 'unavailable', provider: 'linear', reason: 'invalid-binding', message: 'The Linear project no longer matches this project binding.', repoId: repo.id, repoName: repo.displayName, generation })
  }
  return Object.freeze({ status: 'bound', provider: 'linear', repoId: repo.id, repoName: repo.displayName, generation, scopeKey: projectTaskScopeKey({ repoId: repo.id, provider: 'linear', workspaceId: binding.workspaceId, projectId: binding.projectId }), workspaceId: binding.workspaceId, workspaceName: binding.workspaceName, projectId: binding.projectId, projectName: project.name || binding.projectName, ...(project.url || binding.projectUrl ? { projectUrl: project.url ?? binding.projectUrl } : {}), teamIds: Object.freeze((project.teams ?? []).map((team) => team.id)) })
}
