import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '@/tauri'
import type { LinearIssue, LinearWorkflowState } from '@/shared/types'
import type { ProjectTaskScope } from '@/lib/project-task-scope'
import { captureProjectTaskScopeGuard, isProjectTaskScopeGuardCurrent, linearActionMatchesScope, linearIssueMatchesScope } from '@/lib/project-task-scope-guard'
import { isLiveProjectTaskScopeAuthority } from '@/lib/project-task-scope-authority'
import { requestNewSpecFromWorkItem } from '@/lib/sdd-new-spec-entry'
import { linearCreateIssue, linearGetIssue, linearListProjectIssues, linearTeamStates, linearUpdateIssue } from '@/runtime/runtime-linear-client'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

type LinearScope = Extract<ProjectTaskScope, { status: 'bound'; provider: 'linear' }>

export function LockedLinearProjectTasks({ scope }: { scope: LinearScope }): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const liveScopeRef = useRef<ProjectTaskScope>(scope)
  const activeRef = useRef(true)
  liveScopeRef.current = scope
  const [issues, setIssues] = useState<LinearIssue[]>([])
  const [statesByTeam, setStatesByTeam] = useState<Record<string, LinearWorkflowState[]>>({})
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [title, setTitle] = useState('')
  const [creating, setCreating] = useState(false)
  useEffect(() => { activeRef.current = true; return () => { activeRef.current = false } }, [])
  const guardCurrent = useCallback((guard: NonNullable<ReturnType<typeof captureProjectTaskScopeGuard>>): boolean => activeRef.current && isProjectTaskScopeGuardCurrent(guard, liveScopeRef.current) && isLiveProjectTaskScopeAuthority(guard), [])

  const refresh = useCallback(async () => {
    const guard = captureProjectTaskScopeGuard(scope)
    if (!guard) return
    setLoading(true); setError(null); setIssues([])
    try {
      const result = await linearListProjectIssues(settings, scope.projectId, 100, scope.workspaceId)
      if (!guardCurrent(guard)) return
      const exact = result.items.filter((issue) => linearIssueMatchesScope(issue, scope))
      setIssues(exact)
      const teams = [...new Set(exact.map((issue) => issue.team.id).filter((id) => scope.teamIds.includes(id)))]
      const entries = await Promise.all(teams.map(async (teamId) => [teamId, await linearTeamStates(settings, teamId, scope.workspaceId)] as const))
      if (guardCurrent(guard)) setStatesByTeam(Object.fromEntries(entries))
    } catch (cause) {
      if (guardCurrent(guard)) setError(cause instanceof Error ? cause.message : 'Could not load this Linear project.')
    } finally {
      if (guardCurrent(guard)) setLoading(false)
    }
  }, [guardCurrent, scope, settings])

  useEffect(() => { setIssues([]); setStatesByTeam({}); setTitle(''); setCreating(false); void refresh() }, [scope.scopeKey, scope.generation, refresh])

  const createIssue = async (): Promise<void> => {
    const guard = captureProjectTaskScopeGuard(scope)
    const teamId = scope.teamIds[0]
    if (!guard || !teamId || !title.trim() || !linearActionMatchesScope({ workspaceId: scope.workspaceId, projectId: scope.projectId, teamId }, scope)) return
    setCreating(true)
    const result = await linearCreateIssue(settings, { teamId, title: title.trim(), workspaceId: scope.workspaceId, projectId: scope.projectId })
    if (!guardCurrent(guard)) return
    setCreating(false)
    if (!result.ok) { setError(result.error); return }
    if (result.projectId !== scope.projectId || result.teamId !== teamId) { setError('Linear returned an issue outside this project scope.'); return }
    setTitle(''); await refresh()
  }

  const moveIssue = async (issue: LinearIssue, stateId: string): Promise<void> => {
    const guard = captureProjectTaskScopeGuard(scope)
    if (!guard || !linearIssueMatchesScope(issue, scope) || !(statesByTeam[issue.team.id] ?? []).some((state) => state.id === stateId)) return
    const exact = await linearGetIssue(settings, issue.id, scope.workspaceId)
    if (!guardCurrent(guard) || !exact || !linearIssueMatchesScope(exact, scope)) return
    const result = await linearUpdateIssue(settings, issue.id, { stateId }, scope.workspaceId)
    if (!guardCurrent(guard)) return
    if (!result.ok) setError(result.error); else await refresh()
  }

  const authorSpec = async (issue: LinearIssue): Promise<void> => {
    const guard = captureProjectTaskScopeGuard(scope)
    if (!guard || !guardCurrent(guard) || !linearIssueMatchesScope(issue, scope)) return
    const exact = await linearGetIssue(settings, issue.id, scope.workspaceId)
    if (!guardCurrent(guard) || !exact || !linearIssueMatchesScope(exact, scope)) return
    requestNewSpecFromWorkItem({
      repoId: scope.repoId,
      title: exact.title,
      provider: 'linear',
      reference: exact.identifier
    })
  }

  return <div data-testid="sdd-project-scope-f4-guarded-actions" className="flex h-full flex-col overflow-hidden">
    <div className="flex items-center gap-2 border-b p-3"><div className="min-w-0 flex-1"><p className="truncate text-sm font-semibold">{scope.projectName}</p><p className="text-xs text-muted-foreground">{scope.workspaceName}</p></div><Input aria-label="New Linear issue title" value={title} onChange={(event) => setTitle(event.target.value)} className="max-w-xs" /><Button size="sm" disabled={creating || !title.trim() || scope.teamIds.length === 0} onClick={() => void createIssue()}>Create issue</Button><Button size="sm" variant="outline" onClick={() => void refresh()}>Refresh</Button></div>
    {error ? <div role="alert" className="border-b p-3 text-xs text-destructive">{error}</div> : null}
    <div className="min-h-0 flex-1 overflow-auto p-3">{loading ? <p className="text-sm text-muted-foreground">Loading {scope.projectName}…</p> : issues.length === 0 ? <p className="text-sm text-muted-foreground">No issues in {scope.projectName}.</p> : <ul className="space-y-2">{issues.map((issue) => <li key={issue.id} className="flex items-center gap-3 rounded-md border p-3"><button className="min-w-0 flex-1 text-left" onClick={() => void api.shell.openUrl(issue.url)}><span className="mr-2 font-mono text-xs text-muted-foreground">{issue.identifier}</span><span className="text-sm">{issue.title}</span></button><select aria-label={`Status for ${issue.identifier}`} className="h-8 rounded-md border bg-background px-2 text-xs" value="" onChange={(event) => void moveIssue(issue, event.target.value)}><option value="">{issue.state.name}</option>{(statesByTeam[issue.team.id] ?? []).map((state) => <option key={state.id} value={state.id}>{state.name}</option>)}</select><Button size="sm" variant="outline" aria-label={`Author spec from ${issue.identifier}`} onClick={() => void authorSpec(issue)}>New Spec</Button></li>)}</ul>}</div>
  </div>
}
