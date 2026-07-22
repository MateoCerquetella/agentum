import { useEffect, useMemo, useState } from 'react'
import type { LinearProjectSummary, LinearWorkspace, Repo } from '@/shared/types'
import { useAppStore } from '@/store'
import { ProjectBindingEditor } from '@/components/github-projects/ProjectBindingEditor'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { linearListProjects, linearStatus } from '@/runtime/runtime-linear-client'
import { bindingFromLinearSelection } from './project-integrations-section-model'
import { parseTrackerProviderPreference, resolveTrackerProviderPreference, TRACKER_PROVIDER_OPTIONS } from './tracker-provider-options'

export const PROJECT_INTEGRATIONS_SECTION_ID = 'project-integrations'

export function ProjectIntegrationsSection({ repo, updateRepo }: { repo: Repo; updateRepo: (repoId: string, updates: Partial<Repo>) => void }): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const provider = resolveTrackerProviderPreference(repo.trackerProvider)
  const [workspaces, setWorkspaces] = useState<LinearWorkspace[]>([])
  const [workspaceId, setWorkspaceId] = useState(repo.linearProjectBinding?.workspaceId ?? '')
  const [projects, setProjects] = useState<LinearProjectSummary[]>([])
  const [projectId, setProjectId] = useState(repo.linearProjectBinding?.projectId ?? '')
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setWorkspaceId(repo.linearProjectBinding?.workspaceId ?? '')
    setProjectId(repo.linearProjectBinding?.projectId ?? '')
  }, [repo.id, repo.linearProjectBinding])

  useEffect(() => {
    if (provider !== 'linear') return
    let current = true
    setError(null)
    void linearStatus(settings).then((status) => {
      if (!current) return
      setWorkspaces(status.workspaces ?? [])
    }).catch((cause) => current && setError(cause instanceof Error ? cause.message : 'Could not load Linear workspaces.'))
    return () => { current = false }
  }, [provider, settings])

  useEffect(() => {
    if (provider !== 'linear' || !workspaceId) { setProjects([]); return }
    let current = true
    setError(null)
    void linearListProjects(settings, undefined, 100, workspaceId).then((result) => {
      if (current) setProjects(result.items.filter((project) => project.workspaceId === workspaceId || !project.workspaceId))
    }).catch((cause) => current && setError(cause instanceof Error ? cause.message : 'Could not load Linear projects.'))
    return () => { current = false }
  }, [provider, settings, workspaceId])

  const selectedWorkspace = useMemo(() => workspaces.find((item) => item.id === workspaceId), [workspaces, workspaceId])
  const selectedProject = useMemo(() => projects.find((item) => item.id === projectId), [projects, projectId])

  return (
    <section id={PROJECT_INTEGRATIONS_SECTION_ID} data-settings-section={PROJECT_INTEGRATIONS_SECTION_ID} data-testid="sdd-project-scope-f1-settings-relocation" className="space-y-4 scroll-mt-16">
      <div><h3 className="text-sm font-semibold">Integrations</h3><p className="text-xs text-muted-foreground">Choose the one external board owned by {repo.displayName}. Account credentials stay in global Integrations.</p></div>
      <div className="flex items-center justify-between gap-4">
        <Label className="text-xs font-medium">Tracker provider</Label>
        <Select value={provider} onValueChange={(value) => { const next = parseTrackerProviderPreference(value); if (next) updateRepo(repo.id, { trackerProvider: next }) }}>
          <SelectTrigger size="sm" className="h-8 w-[180px] text-xs"><SelectValue /></SelectTrigger>
          <SelectContent>{TRACKER_PROVIDER_OPTIONS.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
        </Select>
      </div>
      {provider === 'github' ? <ProjectBindingEditor key={repo.id} workdir={repo.path} repoId={repo.id} /> : null}
      {provider === 'linear' ? (
        <div data-testid="sdd-project-scope-f2-linear-persistence" className="space-y-3 rounded-md border border-border/50 p-3">
          <Label className="text-xs">Linear workspace</Label>
          <select className="h-8 w-full rounded-md border bg-background px-2 text-xs" value={workspaceId} onChange={(event) => { setWorkspaceId(event.target.value); setProjectId('') }}><option value="">Select a workspace</option>{workspaces.map((workspace) => <option key={workspace.id} value={workspace.id}>{workspace.organizationName}</option>)}</select>
          <Label className="text-xs">Linear project</Label>
          <select className="h-8 w-full rounded-md border bg-background px-2 text-xs" value={projectId} disabled={!workspaceId} onChange={(event) => setProjectId(event.target.value)}><option value="">Select a project</option>{projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}</select>
          <div className="flex gap-2"><Button size="sm" disabled={!selectedWorkspace || !selectedProject} onClick={() => selectedWorkspace && selectedProject && updateRepo(repo.id, { linearProjectBinding: bindingFromLinearSelection(selectedWorkspace, selectedProject) })}>Save Linear board</Button><Button size="sm" variant="outline" onClick={() => { setProjectId(''); updateRepo(repo.id, { linearProjectBinding: null }) }}>Clear</Button></div>
          {repo.linearProjectBinding ? <p className="text-xs text-muted-foreground">Bound to {repo.linearProjectBinding.workspaceName} / {repo.linearProjectBinding.projectName}</p> : null}
          {error ? <p role="alert" className="text-xs text-destructive">{error}</p> : null}
        </div>
      ) : null}
    </section>
  )
}
