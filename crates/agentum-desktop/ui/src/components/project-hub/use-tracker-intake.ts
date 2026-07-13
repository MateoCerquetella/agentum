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
import {
  canDraftIssue,
  canFileIssue,
  deriveFiledGatedRunGate,
  deriveIntentTitle,
  deriveTrackerIntakePhase,
  resolveCreateIssueProvider
} from '@/components/new-workspace/create-issue-intent-model'
import type {
  CreateIssueProvider,
  FiledIssue,
  TrackerIntakePhase
} from '@/components/new-workspace/create-issue-intent-model'
import { resolvePickerProject } from '@/components/new-workspace/work-item-picker-model'
import type { PickerProjectRef } from '@/components/new-workspace/work-item-picker-model'
import { getProjectBinding } from '@/runtime/github-projects-client'
import type { ProjectBindingDto } from '@/runtime/github-projects-client'
import { createGithubIssue, draftGithubIssueBody } from '@/runtime/github-issue-client'
import {
  linearCreateIssue,
  linearListTeams,
  linearStatus
} from '@/runtime/runtime-linear-client'
import type { IssueSideEffectGate } from '@/lib/issue-side-effect-gate'
import { getLinkedWorkItemSuggestedName } from '@/lib/new-workspace'

export type TrackerIntake = {
  /** Which provider the file arm resolves to; `ambiguous` renders the toggle. */
  provider: CreateIssueProvider | 'ambiguous'
  /** The provider a file would actually target (toggle choice applied). */
  effectiveProvider: CreateIssueProvider
  setProviderChoice: (p: CreateIssueProvider) => void
  /** The GitHub Project the tab resolved to (binding > global active), if any. */
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
  bindingVersion
}: {
  repo: Repo
  bindingVersion: number
}): TrackerIntake {
  const openModal = useAppStore((s) => s.openModal)
  const activeProject = useAppStore((s) => s.settings?.githubProjects?.activeProject ?? null)
  // Only the runtime-target field routes the Linear RPC — selecting it (not the
  // whole settings object) keeps the probe from re-running on unrelated writes.
  const activeRuntimeEnvironmentId = useAppStore(
    (s) => s.settings?.activeRuntimeEnvironmentId ?? null
  )
  const linearSettings = useMemo(
    () => ({ activeRuntimeEnvironmentId }),
    [activeRuntimeEnvironmentId]
  )

  // --- Binding read (WorkItemsField precedent): fail-closed null, re-read when
  // ProjectBindingEditor saves (bindingVersion bump). The response's slug also
  // spares the server an `origin` read on draft/create.
  const [binding, setBinding] = useState<ProjectBindingDto | null>(null)
  const [slug, setSlug] = useState<string | null>(null)
  useEffect(() => {
    const workdir = repo.path
    if (!workdir) {
      setBinding(null)
      setSlug(null)
      return
    }
    let cancelled = false
    void getProjectBinding({ workdir })
      .then((res) => {
        if (cancelled) return
        setBinding(res.binding)
        setSlug(res.slug || null)
      })
      .catch(() => {
        if (cancelled) return
        setBinding(null)
        setSlug(null)
      })
    return () => {
      cancelled = true
    }
  }, [repo.path, bindingVersion])

  const resolved = useMemo(
    () => resolvePickerProject({ binding, activeProject }),
    [binding, activeProject]
  )

  // --- Linear probe (CreateIssuePanel precedent): best-effort; any failure
  // keeps the GitHub-only default rather than surfacing an error.
  const [linearConnected, setLinearConnected] = useState(false)
  const [teams, setTeams] = useState<LinearTeam[]>([])
  const [teamId, setTeamId] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    void linearStatus(linearSettings)
      .then((status) => {
        if (cancelled || !status.connected) return
        setLinearConnected(true)
        return linearListTeams(linearSettings).then((list) => {
          if (cancelled) return
          setTeams(list)
          // Sole team auto-selects (the wizard's open-question-2 default).
          if (list.length === 1) setTeamId(list[0].id)
        })
      })
      .catch(() => {
        /* best-effort: stay GitHub-only */
      })
    return () => {
      cancelled = true
    }
  }, [linearSettings])

  const [providerChoice, setProviderChoice] = useState<CreateIssueProvider | null>(null)
  const provider = resolveCreateIssueProvider({ resolved, linearConnected })
  const effectiveProvider: CreateIssueProvider =
    provider === 'ambiguous' ? (providerChoice ?? 'github') : provider

  // --- Intake state (the useComposerState:1519/:1615 handler shapes).
  const [intent, setIntent] = useState('')
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  const [generating, setGenerating] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [filed, setFiled] = useState<FiledIssue | null>(null)

  const busy = generating || submitting
  const phase = deriveTrackerIntakePhase({
    generating,
    submitting,
    error,
    hasBody: body.trim().length > 0,
    filed
  })

  const draft = useCallback(async (): Promise<void> => {
    if (!canDraftIssue(intent, busy)) return
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
    // sit over it as if this draft were already tracked.
    setFiled(null)
    try {
      const { body: drafted } = await draftGithubIssueBody({
        workdir,
        title: seededTitle,
        ...(slug ? { slug } : {})
      })
      setBody(drafted)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not generate a description.')
    } finally {
      setGenerating(false)
    }
  }, [busy, intent, repo.path, slug])

  const fileGithub = useCallback(async (): Promise<void> => {
    const trimmedTitle = title.trim()
    const workdir = repo.path
    if (!workdir) {
      setError('This project has no local workdir to file from.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const created = await createGithubIssue({
        title: trimmedTitle,
        ...(body.trim() ? { body: body.trim() } : {}),
        workdir,
        ...(slug ? { slug } : {})
      })
      // `filed` only from the provider-confirmed response — never before (AC 12).
      setFiled({
        provider: 'github',
        number: created.number,
        url: created.url,
        slug: created.slug,
        title: trimmedTitle
      })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not create the GitHub issue.')
    } finally {
      setSubmitting(false)
    }
  }, [body, repo.path, slug, title])

  const fileLinear = useCallback(async (): Promise<void> => {
    const trimmedTitle = title.trim()
    if (!teamId) {
      setError('Pick a Linear team to file into.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const result = await linearCreateIssue(linearSettings, {
        teamId,
        title: trimmedTitle,
        description: body.trim() || undefined
      })
      if (!result.ok) {
        // Inconclusive/failed never shows "filed" (AC 12) — `filed` unchanged.
        setError(result.error || 'Could not create the Linear issue.')
        return
      }
      setFiled({
        provider: 'linear',
        identifier: result.identifier,
        url: result.url || null,
        title: result.title
      })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not create the Linear issue.')
    } finally {
      setSubmitting(false)
    }
  }, [body, linearSettings, teamId, title])

  const file = useCallback((): void => {
    if (!canFileIssue(title, busy)) return
    if (effectiveProvider === 'linear') {
      void fileLinear()
    } else {
      void fileGithub()
    }
  }, [busy, effectiveProvider, fileGithub, fileLinear, title])

  const gate = deriveFiledGatedRunGate(filed, repo.connectionId)

  const startGatedRun = useCallback((): void => {
    // The gate composes D3 (GitHub-only) and the local-repo precondition; a
    // gated run needs the FRESH worktree the composer creates, so this is the
    // spec-008 pre-armed hop — never a direct `startGatedWork` from here.
    if (!gate.eligible || !filed || filed.provider !== 'github') return
    openModal('new-workspace-composer', {
      linkedWorkItem: { type: 'issue', number: filed.number, title: filed.title, url: filed.url },
      prefilledName: getLinkedWorkItemSuggestedName({ title: filed.title }),
      initialRepoId: repo.id,
      startGatedRun: true,
      telemetrySource: 'sidebar'
    })
  }, [filed, gate.eligible, openModal, repo.id])

  return {
    provider,
    effectiveProvider,
    setProviderChoice,
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
