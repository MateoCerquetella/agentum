// Spec 015 F3: the Tracker tab's intent → issue → gated-run intake, as a thin
// hook over EXISTING seams — `draftGithubIssueBody` / `createGithubIssue`
// (embedded server), `linearCreateIssue` (the 013 F3 native-command client),
// and the spec-008 pre-armed composer hop. It deliberately does NOT reuse
// `useComposerState` (that hook owns a composer's whole lifecycle); it mirrors
// its create-issue handlers so the two panels stay behaviorally aligned while
// this one can outlive the file with a `filed` phase.
import { useCallback, useEffect, useMemo, useState } from 'react'

import { useAppStore } from '@/store'
import type { LinearTeam, Repo } from '@/shared/types'
import type { ProjectTaskScope } from '@/lib/project-task-scope'
import { captureProjectTaskScopeGuard } from '@/lib/project-task-scope-guard'
import { isLiveProjectTaskScopeAuthority } from '@/lib/project-task-scope-authority'
import {
  canDraftIssue,
  canFileIssue,
  deriveDraftGroundingNote,
  deriveFiledGatedRunGate,
  deriveIntentTitle,
  deriveTrackerIntakePhase,
} from '@/components/new-workspace/create-issue-intent-model'
import type {
  CreateIssueProvider,
  DraftGrounding,
  FiledIssue,
  TrackerIntakePhase
} from '@/components/new-workspace/create-issue-intent-model'
import type { PickerProjectRef } from '@/components/new-workspace/work-item-picker-model'
import { createGithubIssue, draftGithubIssueBody } from '@/runtime/github-issue-client'
import {
  linearCreateIssue,
  linearListTeams
} from '@/runtime/runtime-linear-client'
import type { IssueSideEffectGate } from '@/lib/issue-side-effect-gate'
import { getLinkedWorkItemSuggestedName } from '@/lib/new-workspace'

export type TrackerIntake = {
  /** The provider locked by Project Settings. */
  provider: CreateIssueProvider | 'ambiguous'
  /** The provider a file would actually target (toggle choice applied). */
  effectiveProvider: CreateIssueProvider
  setProviderChoice: (p: CreateIssueProvider) => void
  linearConnected: boolean
  /** The exact bound GitHub Project, if any. */
  resolved: PickerProjectRef | null

  intent: string
  setIntent: (value: string) => void
  title: string
  setTitle: (value: string) => void
  body: string
  setBody: (value: string) => void

  phase: TrackerIntakePhase
  error: string | null
  filed: FiledIssue | null
  /** Spec 020 AC 9: the honest note when the draft ran without repo grounding
   *  (SSH repo / unreadable folder) — null when grounded or unknown. */
  groundingNote: string | null
  canDraft: boolean
  canFile: boolean
  draft: () => void
  file: () => void

  teams: LinearTeam[]
  teamId: string | null
  setTeamId: (id: string | null) => void

  /** Gated-run eligibility for the FILED issue — the wizard's own gate. */
  gate: IssueSideEffectGate
  /** The spec-008 pre-armed composer hop (never a direct `startGatedWork`). */
  startGatedRun: () => void
}

export function useTrackerIntake({
  repo,
  scope
}: {
  repo: Repo
  scope: Extract<ProjectTaskScope, { status: 'bound' }>
}): TrackerIntake {
  const openModal = useAppStore((s) => s.openModal)
  // Only the runtime-target field routes the Linear RPC — selecting it (not the
  // whole settings object) keeps the probe from re-running on unrelated writes.
  const activeRuntimeEnvironmentId = useAppStore(
    (s) => s.settings?.activeRuntimeEnvironmentId ?? null
  )
  const linearSettings = useMemo(
    () => ({ activeRuntimeEnvironmentId }),
    [activeRuntimeEnvironmentId]
  )

  const guard = useMemo(() => captureProjectTaskScopeGuard(scope), [scope])!
  const guardCurrent = useCallback(() => isLiveProjectTaskScopeAuthority(guard), [guard])
  const slug = scope.provider === 'github' ? scope.repoSlug : null
  const resolved: PickerProjectRef | null = scope.provider === 'github' && scope.target === 'project'
    ? { owner: scope.owner, ownerType: scope.ownerType, number: scope.projectNumber }
    : null

  // Fetch only teams admitted by the immutable Linear project scope.
  const [teams, setTeams] = useState<LinearTeam[]>([])
  const [teamId, setTeamId] = useState<string | null>(null)
  useEffect(() => {
    setTeams([]); setTeamId(null)
    if (scope.provider !== 'linear') return
    let cancelled = false
    void linearListTeams(linearSettings, scope.workspaceId).then((list) => {
      if (cancelled || !guardCurrent()) return
      const exact = list.filter((team) => team.workspaceId === scope.workspaceId && scope.teamIds.includes(team.id))
      setTeams(exact)
      if (exact.length === 1) setTeamId(exact[0].id)
    }).catch(() => undefined)
    return () => { cancelled = true }
  }, [guardCurrent, linearSettings, scope])

  const provider: CreateIssueProvider = scope.provider
  const effectiveProvider = provider

  // --- Intake state (the useComposerState:1519/:1615 handler shapes).
  const [intent, setIntent] = useState('')
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  const [generating, setGenerating] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [filed, setFiled] = useState<FiledIssue | null>(null)
  // Spec 020 F3 (D4): the server's word on whether repo/wiki context fed the
  // draft — never inferred client-side from connectionId.
  const [grounding, setGrounding] = useState<DraftGrounding | null>(null)
  useEffect(() => { setIntent(''); setTitle(''); setBody(''); setError(null); setFiled(null); setGrounding(null); setGenerating(false); setSubmitting(false) }, [scope.scopeKey, scope.generation])

  const busy = generating || submitting
  const phase = deriveTrackerIntakePhase({
    generating,
    submitting,
    error,
    hasBody: body.trim().length > 0,
    filed
  })

  const draft = useCallback(async (): Promise<void> => {
    if (!canDraftIssue(intent, busy) || !guardCurrent()) return
    const workdir = repo.path
    if (!workdir) {
      setError('This project has no local workdir to draft against.')
      return
    }
    const seededTitle = deriveIntentTitle(intent)
    setTitle(seededTitle)
    setGenerating(true)
    setError(null)
    // Model contract: a new draft is new work — the old "filed" chip must not
    // sit over it as if this draft were already tracked. The grounding flag is
    // per-draft too: a stale note must never describe a fresh draft.
    setFiled(null)
    setGrounding(null)
    try {
      // The draft leg threads the LEARNED slug, not repoId (spec 020 §1.5.1):
      // this route resolves no slug and touches no host — its folder reads are
      // local-by-design, which is exactly what `grounding` reports.
      const res = await draftGithubIssueBody({
        workdir,
        title: seededTitle,
        ...(slug ? { slug } : {})
      })
      if (!guardCurrent()) return
      setBody(res.body)
      setGrounding(res.grounding ?? null)
    } catch (err) {
      if (guardCurrent()) setError(err instanceof Error ? err.message : 'Could not generate a description.')
    } finally {
      if (guardCurrent()) setGenerating(false)
    }
  }, [busy, guardCurrent, intent, repo.path, slug])

  const fileGithub = useCallback(async (): Promise<void> => {
    const trimmedTitle = title.trim()
    const workdir = repo.path
    if (!guardCurrent() || scope.provider !== 'github') return
    if (!workdir) {
      setError('This project has no local workdir to file from.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      // Spec 020 F3: `repoId` is the no-hint robustness path — when the
      // binding read failed earlier (host down) and `slug` is still null, the
      // server resolves the origin on the repo's own host instead of 422ing.
      const created = await createGithubIssue({
        title: trimmedTitle,
        ...(body.trim() ? { body: body.trim() } : {}),
        workdir,
        ...(slug ? { slug } : {}),
        repoId: repo.id
      })
      if (!guardCurrent()) return
      if (created.slug.toLowerCase() !== scope.repoSlug.toLowerCase()) {
        setError('GitHub returned an issue outside this project scope.')
        return
      }
      // `filed` only from the provider-confirmed response — never before (AC 12).
      setFiled({
        provider: 'github',
        number: created.number,
        url: created.url,
        slug: created.slug,
        title: trimmedTitle
      })
    } catch (err) {
      if (guardCurrent()) setError(err instanceof Error ? err.message : 'Could not create the GitHub issue.')
    } finally {
      if (guardCurrent()) setSubmitting(false)
    }
  }, [body, guardCurrent, repo.path, repo.id, scope, slug, title])

  const fileLinear = useCallback(async (): Promise<void> => {
    const trimmedTitle = title.trim()
    if (!teamId || !guardCurrent() || scope.provider !== 'linear' || !scope.teamIds.includes(teamId)) {
      setError('Pick a Linear team to file into.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const result = await linearCreateIssue(linearSettings, {
        teamId,
        title: trimmedTitle,
        description: body.trim() || undefined,
        workspaceId: scope.workspaceId,
        projectId: scope.projectId
      })
      if (!guardCurrent()) return
      if (!result.ok) {
        // Inconclusive/failed never shows "filed" (AC 12) — `filed` unchanged.
        setError(result.error || 'Could not create the Linear issue.')
        return
      }
      if (result.projectId !== scope.projectId || result.teamId !== teamId) {
        setError('Linear returned an issue outside this project scope.')
        return
      }
      setFiled({
        provider: 'linear',
        identifier: result.identifier,
        url: result.url || null,
        title: result.title
      })
    } catch (err) {
      if (guardCurrent()) setError(err instanceof Error ? err.message : 'Could not create the Linear issue.')
    } finally {
      if (guardCurrent()) setSubmitting(false)
    }
  }, [body, guardCurrent, linearSettings, scope, teamId, title])

  const file = useCallback((): void => {
    if (!canFileIssue(title, busy) || !guardCurrent()) return
    if (effectiveProvider === 'linear') {
      void fileLinear()
    } else {
      void fileGithub()
    }
  }, [busy, effectiveProvider, fileGithub, fileLinear, guardCurrent, title])

  const gate = deriveFiledGatedRunGate(filed, repo.connectionId)

  // The host label explains why repo grounding was unavailable (presentation
  // only); gated-run support itself is host-aware and no longer local-only.
  const hostLabel = useAppStore((s) =>
    repo.connectionId ? (s.sshTargetLabels.get(repo.connectionId) ?? 'a remote host') : null
  )
  const groundingNote = deriveDraftGroundingNote(grounding, hostLabel)

  const startGatedRun = useCallback((): void => {
    // The gate composes D3 (GitHub-only); a gated run needs the FRESH worktree
    // the composer creates, so this is the
    // spec-008 pre-armed hop — never a direct `startGatedWork` from here.
    if (!gate.eligible || !filed || filed.provider !== 'github' || !guardCurrent()) return
    openModal('new-workspace-composer', {
      linkedWorkItem: { type: 'issue', number: filed.number, title: filed.title, url: filed.url },
      prefilledName: getLinkedWorkItemSuggestedName({ title: filed.title }),
      initialRepoId: repo.id,
      startGatedRun: true,
      telemetrySource: 'sidebar',
      requiredProjectTaskScope: guard
    })
  }, [filed, gate.eligible, guard, guardCurrent, openModal, repo.id])

  return {
    provider,
    effectiveProvider,
    setProviderChoice: () => undefined,
    /** #379: surfaced so the panel can SHOW the Linear connection state. */
    linearConnected: scope.provider === 'linear',
    resolved,
    intent,
    setIntent,
    title,
    setTitle,
    body,
    setBody,
    phase,
    error,
    filed,
    groundingNote,
    canDraft: canDraftIssue(intent, busy),
    canFile: canFileIssue(title, busy),
    draft: () => void draft(),
    file,
    teams,
    teamId,
    setTeamId,
    gate,
    startGatedRun
  }
}
