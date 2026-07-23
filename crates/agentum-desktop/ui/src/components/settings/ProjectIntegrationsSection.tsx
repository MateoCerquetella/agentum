import { useCallback, useEffect, useMemo, useState } from 'react'
import { Github, Loader2 } from 'lucide-react'
import type { LinearProjectSummary, LinearTeam, LinearWorkspace, Repo } from '@/shared/types'
import type {
  ProjectTrackerConfig,
  ProjectTrackerProvider
} from '@/shared/project-tracker-config'
import {
  PROJECT_TRACKER_SCHEMA_VERSION,
  unconfiguredProjectTrackerConfig
} from '@/shared/project-tracker-config'
import { useAppStore } from '@/store'
import { ProjectBindingEditor } from '@/components/github-projects/ProjectBindingEditor'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui/select'
import {
  linearListProjects,
  linearListTeams,
  linearStatus
} from '@/runtime/runtime-linear-client'
import { getServerRepoSlug } from '@/runtime/server-repo-client'

export const PROJECT_INTEGRATIONS_SECTION_ID = 'project-integrations'

type ProviderChoice = ProjectTrackerProvider | 'none'

function nextConfig(
  repoId: string,
  current: ProjectTrackerConfig | null | undefined,
  provider: ProjectTrackerProvider | null
): ProjectTrackerConfig {
  return {
    schemaVersion: PROJECT_TRACKER_SCHEMA_VERSION,
    repoId,
    revision: current?.revision ?? 0,
    provider,
    taskPreferences: current?.taskPreferences ?? {},
    provenance: 'configured'
  }
}

export function ProjectIntegrationsSection({ repo }: { repo: Repo }): React.JSX.Element {
  const settings = useAppStore((state) => state.settings)
  const config = useAppStore((state) => state.projectTrackerConfigByRepo[repo.id])
  const loadStatus = useAppStore(
    (state) => state.projectTrackerLoadStatusByRepo[repo.id] ?? 'idle'
  )
  const storeError = useAppStore((state) => state.projectTrackerErrorByRepo[repo.id])
  const migrationConflict = useAppStore(
    (state) => state.projectTrackerMigrationConflictByRepo[repo.id]
  )
  const saving = useAppStore((state) => state.projectTrackerSavingByRepo[repo.id] ?? false)
  const loadConfig = useAppStore((state) => state.loadProjectTrackerConfig)
  const saveConfig = useAppStore((state) => state.saveProjectTrackerConfig)

  const [providerDraft, setProviderDraft] = useState<ProviderChoice>('none')
  const [workspaces, setWorkspaces] = useState<LinearWorkspace[]>([])
  const [workspaceId, setWorkspaceId] = useState('')
  const [projects, setProjects] = useState<LinearProjectSummary[]>([])
  const [projectId, setProjectId] = useState('')
  const [teams, setTeams] = useState<LinearTeam[]>([])
  const [teamId, setTeamId] = useState('')
  const [loadingLinear, setLoadingLinear] = useState(false)
  const [localError, setLocalError] = useState<string | null>(null)

  useEffect(() => {
    void loadConfig(repo.id).catch(() => undefined)
  }, [loadConfig, repo.id])

  useEffect(() => {
    const provider = config?.provider ?? 'none'
    setProviderDraft(provider)
    setWorkspaceId(config?.linear?.workspaceId ?? '')
    setProjectId(config?.linear?.scope?.kind === 'project' ? config.linear.scope.id : '')
    setTeamId(config?.linear?.teamId ?? '')
    setLocalError(null)
  }, [config, repo.id])

  useEffect(() => {
    if (providerDraft !== 'linear') return
    let current = true
    setLoadingLinear(true)
    setLocalError(null)
    void linearStatus(settings)
      .then((status) => {
        if (!current) return
        setWorkspaces(status.workspaces ?? [])
        if (!status.connected) setLocalError('Connect Linear in global Integrations first.')
      })
      .catch((cause) => {
        if (current) {
          setLocalError(
            cause instanceof Error ? cause.message : 'Could not load Linear workspaces.'
          )
        }
      })
      .finally(() => {
        if (current) setLoadingLinear(false)
      })
    return () => {
      current = false
    }
  }, [providerDraft, settings])

  useEffect(() => {
    if (providerDraft !== 'linear' || !workspaceId) {
      setProjects([])
      setTeams([])
      return
    }
    let current = true
    setLoadingLinear(true)
    setLocalError(null)
    void Promise.all([
      linearListProjects(settings, undefined, 100, workspaceId),
      linearListTeams(settings, workspaceId)
    ])
      .then(([projectResult, teamResult]) => {
        if (!current) return
        setProjects(
          projectResult.items.filter(
            (project) => project.workspaceId === workspaceId || !project.workspaceId
          )
        )
        setTeams(
          teamResult.filter((team) => team.workspaceId === workspaceId || !team.workspaceId)
        )
      })
      .catch((cause) => {
        if (current) {
          setLocalError(
            cause instanceof Error ? cause.message : 'Could not load Linear projects.'
          )
        }
      })
      .finally(() => {
        if (current) setLoadingLinear(false)
      })
    return () => {
      current = false
    }
  }, [providerDraft, settings, workspaceId])

  const selectedWorkspace = useMemo(
    () => workspaces.find((item) => item.id === workspaceId),
    [workspaces, workspaceId]
  )
  const selectedProject = useMemo(
    () => projects.find((item) => item.id === projectId),
    [projects, projectId]
  )

  const persistProvider = useCallback(
    async (provider: ProviderChoice): Promise<void> => {
      setProviderDraft(provider)
      setLocalError(null)
      try {
        if (provider === 'none') {
          await saveConfig(repo.id, unconfiguredProjectTrackerConfig(repo.id, config))
          return
        }
        if (provider === 'linear') return
        const repositorySlug =
          config?.provider === 'github' && config.github?.repositorySlug
            ? config.github.repositorySlug
            : (await getServerRepoSlug(repo.id)).slug
        await saveConfig(repo.id, {
          ...nextConfig(repo.id, config, 'github'),
          github: {
            repositorySlug,
            ...(config?.provider === 'github' && config.github?.projectBinding
              ? { projectBinding: config.github.projectBinding }
              : {})
          }
        })
      } catch (cause) {
        setProviderDraft(config?.provider ?? 'none')
        setLocalError(cause instanceof Error ? cause.message : 'Could not save tracker settings.')
      }
    },
    [config, repo.id, saveConfig]
  )

  const saveLinear = useCallback(async (): Promise<void> => {
    if (!selectedWorkspace || !selectedProject) return
    setLocalError(null)
    try {
      await saveConfig(repo.id, {
        ...nextConfig(repo.id, config, 'linear'),
        linear: {
          workspaceId: selectedWorkspace.id,
          ...(teamId ? { teamId } : {}),
          scope: { kind: 'project', id: selectedProject.id }
        }
      })
    } catch (cause) {
      setLocalError(cause instanceof Error ? cause.message : 'Could not save the Linear tracker.')
    }
  }, [config, repo.id, saveConfig, selectedProject, selectedWorkspace, teamId])

  const restoreGithubRepositoryFallback = useCallback(async (): Promise<void> => {
    try {
      const latest = await loadConfig(repo.id, { force: true })
      const repositorySlug =
        config?.github?.repositorySlug ?? (await getServerRepoSlug(repo.id)).slug
      await saveConfig(repo.id, {
        ...nextConfig(repo.id, latest, 'github'),
        github: { repositorySlug }
      })
    } catch (cause) {
      setLocalError(
        cause instanceof Error ? cause.message : 'Could not restore repository issue fallback.'
      )
    }
  }, [config?.github?.repositorySlug, loadConfig, repo.id, saveConfig])

  const error = localError ?? storeError ?? null
  const providerLabel = config?.provider
    ? config.provider === 'github'
      ? config.github?.projectBinding?.projectTitle ?? config.github?.repositorySlug ?? 'GitHub'
      : selectedProject?.name ?? 'Linear'
    : 'No tracker configured'

  return (
    <section
      id={PROJECT_INTEGRATIONS_SECTION_ID}
      data-settings-section={PROJECT_INTEGRATIONS_SECTION_ID}
      className="scroll-mt-16 space-y-4"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="text-sm font-semibold">Project tracker</h3>
          <p className="mt-1 max-w-xl text-xs text-muted-foreground">
            One configured provider controls issue lists, creation, and automation for{' '}
            {repo.displayName}. Credentials remain in global Integrations.
          </p>
        </div>
        <span className="rounded-full border border-border px-2.5 py-1 font-mono text-[10px] text-muted-foreground">
          {providerLabel}
        </span>
      </div>

      <div className="grid grid-cols-1 items-end gap-3 sm:grid-cols-[minmax(0,1fr)_220px]">
        <div className="space-y-1">
          <Label className="text-xs font-medium">Tracker provider</Label>
          <p className="text-[11px] text-muted-foreground">
            New and existing issue workflows stay disabled until this target is saved.
          </p>
        </div>
        <Select
          value={providerDraft}
          disabled={loadStatus === 'loading' || saving}
          onValueChange={(value) => void persistProvider(value as ProviderChoice)}
        >
          <SelectTrigger size="sm" className="h-8 w-full text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="none">None</SelectItem>
            <SelectItem value="github">GitHub</SelectItem>
            <SelectItem value="linear">Linear</SelectItem>
          </SelectContent>
        </Select>
      </div>

      {loadStatus === 'loading' ? (
        <div className="flex items-center gap-2 rounded-md border border-border p-3 text-xs text-muted-foreground">
          <Loader2 className="size-3.5 animate-spin" />
          Loading tracker configuration…
        </div>
      ) : null}

      {providerDraft === 'github' && config?.provider === 'github' ? (
        <div className="space-y-3 rounded-md border border-border/70 p-3">
          <div className="flex items-center gap-2 text-xs font-medium">
            <Github className="size-3.5" />
            GitHub issue source
          </div>
          <p className="text-[11px] text-muted-foreground">
            Without a Project binding, issue lists fall back to open issues from{' '}
            <span className="font-mono text-foreground">{config.github?.repositorySlug}</span>.
          </p>
          <ProjectBindingEditor
            key={`${repo.id}:${config.revision}`}
            workdir={repo.path}
            repoId={repo.id}
            onBound={() => void loadConfig(repo.id, { force: true }).catch(() => undefined)}
            onUnbound={() => void restoreGithubRepositoryFallback()}
          />
        </div>
      ) : null}

      {providerDraft === 'linear' ? (
        <div className="space-y-3 rounded-md border border-border/70 p-3">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <label className="space-y-1.5 text-xs">
              <span className="font-medium">Workspace</span>
              <select
                className="h-8 w-full rounded-md border bg-background px-2 text-xs"
                value={workspaceId}
                disabled={loadingLinear}
                onChange={(event) => {
                  setWorkspaceId(event.target.value)
                  setProjectId('')
                  setTeamId('')
                }}
              >
                <option value="">Select a workspace</option>
                {workspaces.map((workspace) => (
                  <option key={workspace.id} value={workspace.id}>
                    {workspace.organizationName}
                  </option>
                ))}
              </select>
            </label>
            <label className="space-y-1.5 text-xs">
              <span className="font-medium">Project</span>
              <select
                className="h-8 w-full rounded-md border bg-background px-2 text-xs"
                value={projectId}
                disabled={!workspaceId || loadingLinear}
                onChange={(event) => {
                  const nextProjectId = event.target.value
                  setProjectId(nextProjectId)
                  const projectTeams =
                    projects.find((project) => project.id === nextProjectId)?.teams ?? []
                  if (projectTeams.length === 1) setTeamId(projectTeams[0].id)
                }}
              >
                <option value="">Select a project</option>
                {projects.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <label className="block space-y-1.5 text-xs">
            <span className="font-medium">Issue team (optional)</span>
            <select
              className="h-8 w-full rounded-md border bg-background px-2 text-xs sm:max-w-[calc(50%-0.375rem)]"
              value={teamId}
              disabled={!workspaceId || loadingLinear}
              onChange={(event) => setTeamId(event.target.value)}
            >
              <option value="">Choose when creating an issue</option>
              {teams.map((team) => (
                <option key={team.id} value={team.id}>
                  {team.name} ({team.key})
                </option>
              ))}
            </select>
          </label>
          <Button
            size="sm"
            disabled={!selectedWorkspace || !selectedProject || saving || loadingLinear}
            onClick={() => void saveLinear()}
          >
            {saving ? <Loader2 className="mr-1.5 size-3.5 animate-spin" /> : null}
            Save Linear tracker
          </Button>
        </div>
      ) : null}

      {migrationConflict ? (
        <p role="status" className="text-xs text-amber-600 dark:text-amber-400">
          Migration needs review: {migrationConflict}
        </p>
      ) : null}
      {error ? (
        <div className="flex flex-wrap items-center gap-2" role="alert">
          <p className="min-w-0 flex-1 text-xs text-destructive">{error}</p>
          {loadStatus === 'error' ? (
            <Button
              size="sm"
              variant="outline"
              onClick={() => void loadConfig(repo.id, { force: true }).catch(() => undefined)}
            >
              Retry
            </Button>
          ) : null}
        </div>
      ) : null}
    </section>
  )
}
