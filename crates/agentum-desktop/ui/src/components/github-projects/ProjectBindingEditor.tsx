// The shared Projects v2 board-binding editor (spec 010 F1, D7): pick a
// project (reusing the desktop's registered READ commands — writes are
// server-side), discover its Status field through the embedded server, render
// the resolved five-phase mapping as per-phase selects (FellBack phases get a
// visible D5 hint; a refusal renders empty selects — a prompt to finish
// manually, never a dead end), then Save/Re-discover/Unbind. Mounted by
// Settings → Integrations today; F3's wizard step mounts this SAME component.
import { api } from '@/tauri'
import { useCallback, useEffect, useReducer, useState } from 'react'
import { CheckCircle2, LoaderCircle, RefreshCw, Unlink } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { GhAuthErrorHelp } from '@/components/github-project/GhAuthErrorHelp'
import { useMountedRef } from '@/hooks/useMountedRef'
import {
  BOARD_PHASE_LABELS,
  EDITABLE_BOARD_PHASES,
  EMPTY_SELECTION,
  fallbackHints,
  mappingComplete,
  OPTIONAL_BOARD_PHASES,
  optionNamesForSelection,
  reduceBindingSelection,
  selectionForRebind,
  selectionFromResolved,
  type BindingSelection
} from '@/lib/github-projects-binding'
import {
  GithubProjectsBindingError,
  deleteProjectBinding,
  discoverProjectStatus,
  getProjectBinding,
  putProjectBinding,
  type DiscoverProjectStatusResponse,
  type ProjectBindingDto
} from '@/runtime/github-projects-client'
import type {
  GitHubProjectOwnerType,
  GitHubProjectSummary,
  ListAccessibleProjectsResult,
  ResolveProjectRefResult
} from '@/shared/github-project-types'

type PickedProject = {
  owner: string
  ownerType: GitHubProjectOwnerType
  number: number
  title: string
}

type BindError = { code?: string; message: string }

function toBindError(err: unknown): BindError {
  if (err instanceof GithubProjectsBindingError) {
    return { code: err.code, message: err.message }
  }
  return { message: err instanceof Error ? err.message : String(err) }
}

export function ProjectBindingEditor({
  workdir,
  slug,
  repoId,
  onBound,
  onUnbound
}: {
  workdir: string
  slug?: string
  /** Spec 020 F3: the registered repo's id — the server resolves the slug on
   *  that repo's own host, so SSH repos bind too. Absent = local (pre-020). */
  repoId?: string
  onBound?: (binding: ProjectBindingDto, repositorySlug: string) => void
  /** Fired only after the binding DELETE succeeds. */
  onUnbound?: () => void
}): React.JSX.Element {
  const mounted = useMountedRef()
  const [loaded, setLoaded] = useState(false)
  const [binding, setBinding] = useState<ProjectBindingDto | null>(null)
  const [picked, setPicked] = useState<PickedProject | null>(null)
  const [discovery, setDiscovery] = useState<DiscoverProjectStatusResponse | null>(null)
  const [selection, dispatchSelection] = useReducer(reduceBindingSelection, EMPTY_SELECTION)
  const [doneClosesIssue, setDoneClosesIssue] = useState(true)
  const [error, setError] = useState<BindError | null>(null)
  const [busy, setBusy] = useState<'discover' | 'save' | 'unbind' | null>(null)
  const [saved, setSaved] = useState(false)
  // Project pick (unbound state): the accessible-projects list + paste input.
  const [projects, setProjects] = useState<GitHubProjectSummary[] | null>(null)
  const [projectsError, setProjectsError] = useState<BindError | null>(null)
  const [pasteInput, setPasteInput] = useState('')

  // Fresh load per repo: the binding is keyed by slug, so switching the
  // selected repo re-reads it.
  useEffect(() => {
    setLoaded(false)
    setBinding(null)
    setPicked(null)
    setDiscovery(null)
    setError(null)
    setSaved(false)
    dispatchSelection({ type: 'reset', selection: EMPTY_SELECTION })
    let cancelled = false
    void getProjectBinding({ workdir, slug, ...(repoId ? { repoId } : {}) })
      .then((res) => {
        if (cancelled || !mounted.current) return
        setBinding(res.binding)
        setDoneClosesIssue(res.binding?.doneClosesIssue ?? true)
        setLoaded(true)
      })
      .catch((err: unknown) => {
        if (cancelled || !mounted.current) return
        setError(toBindError(err))
        setLoaded(true)
      })
    return () => {
      cancelled = true
    }
  }, [workdir, slug, repoId, mounted])

  // The accessible-projects list loads only for the unbound pick UI — bound
  // repos never need it, and re-binding starts from the stored project ref.
  useEffect(() => {
    if (!loaded || binding || projects) return
    let cancelled = false
    void (api.gh.listAccessibleProjects() as Promise<ListAccessibleProjectsResult>)
      .then((res) => {
        if (cancelled || !mounted.current) return
        // Paired positive guards: with this tsconfig's `strict: false`, only
        // then-branch discriminant narrowing applies (else/exclusion doesn't).
        if (res.ok) {
          setProjects(res.projects)
          setProjectsError(null)
        }
        if (res.ok === false) {
          setProjects([])
          setProjectsError({ code: res.error.type, message: res.error.message })
        }
      })
      .catch((err: unknown) => {
        if (cancelled || !mounted.current) return
        setProjects([])
        setProjectsError(toBindError(err))
      })
    return () => {
      cancelled = true
    }
  }, [loaded, binding, projects, mounted])

  const runDiscovery = useCallback(
    async (project: PickedProject, stored?: ProjectBindingDto | null) => {
      setBusy('discover')
      setError(null)
      setSaved(false)
      try {
        const res = await discoverProjectStatus({
          owner: project.owner,
          ownerType: project.ownerType,
          number: project.number
        })
        if (!mounted.current) return
        setPicked({ ...project, title: res.title || project.title })
        setDiscovery(res)
        // Re-discovery on a bound repo keeps stored (possibly hand-edited)
        // option ids that still exist; a fresh bind seeds from `resolved`.
        const next: BindingSelection = stored
          ? selectionForRebind(stored.statusMapping, res.resolved, res.options)
          : selectionFromResolved(res.resolved)
        dispatchSelection({ type: 'reset', selection: next })
        if (res.resolved === null) {
          setError({
            message:
              `Could not auto-map ${res.unmappedPhases.join(', ')} onto this board's ` +
              `columns — pick each column below to finish binding.`
          })
        }
      } catch (err) {
        if (!mounted.current) return
        setDiscovery(null)
        setError(toBindError(err))
      } finally {
        if (mounted.current) setBusy(null)
      }
    },
    [mounted]
  )

  const handlePaste = useCallback(async () => {
    const input = pasteInput.trim()
    if (!input) return
    setBusy('discover')
    setError(null)
    try {
      const res = (await api.gh.resolveProjectRef({ input })) as ResolveProjectRefResult
      if (!mounted.current) return
      // Paired positive guards — see the narrowing note in the list effect.
      if (res.ok === false) {
        setBusy(null)
        setError({ code: res.error.type, message: res.error.message })
      }
      if (res.ok) {
        setPasteInput('')
        await runDiscovery(
          { owner: res.owner, ownerType: res.ownerType, number: res.number, title: res.title },
          binding
        )
      }
    } catch (err) {
      if (!mounted.current) return
      setBusy(null)
      setError(toBindError(err))
    }
  }, [pasteInput, binding, runDiscovery, mounted])

  const handleRediscover = useCallback(() => {
    if (!binding) return
    if (binding.projectOwner && binding.projectOwnerType && binding.projectNumber != null) {
      void runDiscovery(
        {
          owner: binding.projectOwner,
          ownerType: binding.projectOwnerType === 'organization' ? 'organization' : 'user',
          number: binding.projectNumber,
          title: binding.projectTitle ?? ''
        },
        binding
      )
    } else {
      // A binding stored without its project ref (older writer) can't
      // re-discover blind — fall back to the pick UI.
      setBinding(null)
    }
  }, [binding, runDiscovery])

  const handleSave = useCallback(async () => {
    if (!discovery || !picked || !mappingComplete(selection)) return
    setBusy('save')
    setError(null)
    try {
      const res = await putProjectBinding({
        workdir,
        ...(slug ? { slug } : {}),
        ...(repoId ? { repoId } : {}),
        projectId: discovery.projectId,
        statusFieldId: discovery.statusFieldId,
        statusMapping: { ...selection },
        doneClosesIssue,
        projectTitle: discovery.title,
        projectOwner: picked.owner,
        projectOwnerType: picked.ownerType,
        projectNumber: picked.number,
        optionNames: optionNamesForSelection(selection, discovery.options)
      })
      if (!mounted.current) return
      setBinding(res.binding)
      setDiscovery(null)
      setPicked(null)
      setSaved(true)
      onBound?.(res.binding, res.slug)
    } catch (err) {
      if (!mounted.current) return
      setError(toBindError(err))
    } finally {
      if (mounted.current) setBusy(null)
    }
  }, [discovery, picked, selection, doneClosesIssue, workdir, slug, repoId, onBound, mounted])

  // Flip the D1 knob on an already-bound repo without a re-discover: the
  // stored binding carries every field the PUT needs.
  const handleToggleDoneCloses = useCallback(
    async (value: boolean) => {
      if (!binding) return
      setDoneClosesIssue(value)
      setError(null)
      try {
        const res = await putProjectBinding({
          workdir,
          ...(slug ? { slug } : {}),
          ...(repoId ? { repoId } : {}),
          projectId: binding.projectId,
          statusFieldId: binding.statusFieldId,
          statusMapping: binding.statusMapping,
          doneClosesIssue: value,
          ...(binding.projectTitle ? { projectTitle: binding.projectTitle } : {}),
          ...(binding.projectOwner ? { projectOwner: binding.projectOwner } : {}),
          ...(binding.projectOwnerType ? { projectOwnerType: binding.projectOwnerType } : {}),
          ...(binding.projectNumber != null ? { projectNumber: binding.projectNumber } : {}),
          ...(binding.optionNames ? { optionNames: binding.optionNames } : {})
        })
        if (!mounted.current) return
        setBinding(res.binding)
      } catch (err) {
        if (!mounted.current) return
        setDoneClosesIssue(!value)
        setError(toBindError(err))
      }
    },
    [binding, workdir, slug, repoId, mounted]
  )

  const handleUnbind = useCallback(async () => {
    setBusy('unbind')
    setError(null)
    try {
      await deleteProjectBinding({ workdir, slug, ...(repoId ? { repoId } : {}) })
      if (!mounted.current) return
      setBinding(null)
      setDiscovery(null)
      setPicked(null)
      setSaved(false)
      dispatchSelection({ type: 'reset', selection: EMPTY_SELECTION })
      onUnbound?.()
    } catch (err) {
      if (!mounted.current) return
      setError(toBindError(err))
    } finally {
      if (mounted.current) setBusy(null)
    }
  }, [workdir, slug, repoId, onUnbound, mounted])

  if (!loaded) {
    return (
      <div className="flex items-center gap-2 py-2 text-xs text-muted-foreground">
        <LoaderCircle className="size-3.5 animate-spin" />
        Loading board binding…
      </div>
    )
  }

  const authError =
    error && (error.code === 'scope_missing' || error.code === 'auth_required') ? error : null
  const hints = discovery ? fallbackHints(discovery.resolved) : {}

  const doneClosesToggle = (
    <label className="flex items-center justify-between gap-3">
      <span className="text-xs text-muted-foreground">
        Close the issue at Done (reopen on a later In Progress)
      </span>
      <button
        role="switch"
        aria-checked={doneClosesIssue}
        onClick={() =>
          binding && !discovery
            ? void handleToggleDoneCloses(!doneClosesIssue)
            : setDoneClosesIssue(!doneClosesIssue)
        }
        className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors ${
          doneClosesIssue ? 'bg-foreground' : 'bg-muted-foreground/30'
        }`}
      >
        <span
          className={`inline-block h-3.5 w-3.5 transform rounded-full bg-background shadow-sm transition-transform ${
            doneClosesIssue ? 'translate-x-4' : 'translate-x-0.5'
          }`}
        />
      </button>
    </label>
  )

  // ── The mapping editor (fresh discovery OR re-discovery) ────────────────
  if (discovery) {
    return (
      <div className="space-y-2.5">
        <p className="text-xs text-muted-foreground">
          <span className="font-medium text-foreground">{discovery.title}</span>
          {picked ? ` · ${picked.owner} #${picked.number}` : null}
        </p>
        {error && !authError ? <p className="text-xs text-destructive">{error.message}</p> : null}
        <div className="grid grid-cols-1 gap-2">
          {EDITABLE_BOARD_PHASES.map((phase) => {
            const optional = OPTIONAL_BOARD_PHASES.includes(phase)
            return (
              <div key={phase} className="grid grid-cols-[110px_minmax(0,1fr)] items-center gap-2">
                <span className="text-[11px] font-medium text-muted-foreground">
                  {BOARD_PHASE_LABELS[phase]}
                  {optional ? <span className="text-muted-foreground/50"> (opt)</span> : null}
                </span>
                <div className="min-w-0">
                  <select
                    value={selection[phase]}
                    onChange={(e) =>
                      dispatchSelection({ type: 'set', phase, optionId: e.target.value })
                    }
                    className="h-8 w-full min-w-0 rounded-md border border-input bg-background px-2 text-xs text-foreground"
                  >
                    {/* #379: In Review is optional — an empty pick is valid and
                        folds onto In Progress, so its placeholder says so. */}
                    <option value="">
                      {optional ? 'Not tracked (uses In Progress)' : 'Pick a column…'}
                    </option>
                    {discovery.options.map((o) => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                      </option>
                    ))}
                  </select>
                  {!optional && hints[phase] ? (
                    <p className="mt-0.5 text-[11px] text-amber-700 dark:text-amber-300">
                      {hints[phase]}
                    </p>
                  ) : null}
                  {optional && phase === 'inReview' ? (
                    <p className="mt-0.5 text-[11px] text-muted-foreground/70">
                      Where a card moves when its PR opens. Leave unset to keep it in In Progress.
                    </p>
                  ) : null}
                </div>
              </div>
            )
          })}
        </div>
        {doneClosesToggle}
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void handleSave()}
            disabled={busy !== null || !mappingComplete(selection)}
          >
            {busy === 'save' ? (
              <>
                <LoaderCircle className="mr-1.5 size-3.5 animate-spin" />
                Saving…
              </>
            ) : (
              'Save binding'
            )}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setDiscovery(null)
              setPicked(null)
              setError(null)
            }}
            disabled={busy !== null}
          >
            Cancel
          </Button>
        </div>
      </div>
    )
  }

  // ── Bound summary ────────────────────────────────────────────────────────
  if (binding) {
    const mappedNames = binding.optionNames
    return (
      <div className="space-y-2.5">
        <div className="flex items-center gap-2">
          <p className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
            Bound to{' '}
            <span className="font-medium text-foreground">
              {binding.projectTitle || binding.projectId}
            </span>
            {binding.projectOwner && binding.projectNumber != null
              ? ` · ${binding.projectOwner} #${binding.projectNumber}`
              : null}
          </p>
          {saved ? (
            <span className="flex shrink-0 items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400">
              <CheckCircle2 className="size-3.5" />
              Saved
            </span>
          ) : null}
        </div>
        {mappedNames ? (
          <p className="text-[11px] text-muted-foreground/70">
            {EDITABLE_BOARD_PHASES
              // #379: an unmapped optional phase (In Review with no column) is
              // omitted from the summary rather than shown as "→ ?".
              .filter((phase) => !OPTIONAL_BOARD_PHASES.includes(phase) || mappedNames[phase])
              .map((phase) => `${BOARD_PHASE_LABELS[phase]} → ${mappedNames[phase] || '?'}`)
              .join(' · ')}
          </p>
        ) : null}
        {doneClosesToggle}
        {authError ? (
          <GhAuthErrorHelp
            error={{ type: authError.code as 'auth_required' | 'scope_missing', message: authError.message }}
          />
        ) : error ? (
          <p className="text-xs text-destructive">{error.message}</p>
        ) : null}
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={handleRediscover}
            disabled={busy !== null}
          >
            {busy === 'discover' ? (
              <LoaderCircle className="mr-1.5 size-3.5 animate-spin" />
            ) : (
              <RefreshCw className="mr-1.5 size-3.5" />
            )}
            Re-discover
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void handleUnbind()}
            disabled={busy !== null}
          >
            <Unlink className="mr-1.5 size-3.5" />
            Unbind
          </Button>
        </div>
      </div>
    )
  }

  // ── Unbound: pick a project ──────────────────────────────────────────────
  return (
    <div className="space-y-2.5">
      <p className="text-xs text-muted-foreground">
        Bind a GitHub Projects v2 board so gated runs move its cards (and close issues at Done).
      </p>
      {authError ? (
        <GhAuthErrorHelp
          error={{ type: authError.code as 'auth_required' | 'scope_missing', message: authError.message }}
        />
      ) : error ? (
        <p className="text-xs text-destructive">{error.message}</p>
      ) : null}
      {projectsError && !error ? (
        projectsError.code === 'scope_missing' || projectsError.code === 'auth_required' ? (
          <GhAuthErrorHelp
            error={{
              type: projectsError.code as 'auth_required' | 'scope_missing',
              message: projectsError.message
            }}
          />
        ) : (
          <p className="text-xs text-destructive">{projectsError.message}</p>
        )
      ) : null}
      {projects === null ? (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <LoaderCircle className="size-3.5 animate-spin" />
          Loading your projects…
        </div>
      ) : projects.length > 0 ? (
        <select
          value=""
          disabled={busy !== null}
          onChange={(e) => {
            const p = projects.find(
              (candidate) =>
                `${candidate.ownerType}:${candidate.owner}:${candidate.number}` === e.target.value
            )
            if (p) {
              void runDiscovery(
                { owner: p.owner, ownerType: p.ownerType, number: p.number, title: p.title },
                null
              )
            }
          }}
          className="h-8 w-full min-w-0 rounded-md border border-input bg-background px-2 text-xs text-foreground"
        >
          <option value="">
            {busy === 'discover' ? 'Discovering…' : 'Choose a project…'}
          </option>
          {projects.map((p) => (
            <option
              key={`${p.ownerType}:${p.owner}:${p.number}`}
              value={`${p.ownerType}:${p.owner}:${p.number}`}
            >
              {p.owner} / {p.title} (#{p.number})
            </option>
          ))}
        </select>
      ) : null}
      <div className="flex items-center gap-2">
        <Input
          value={pasteInput}
          onChange={(e) => setPasteInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void handlePaste()
          }}
          placeholder="…or paste a project URL / owner/number"
          className="h-8 text-xs"
          disabled={busy !== null}
        />
        <Button
          variant="outline"
          size="sm"
          onClick={() => void handlePaste()}
          disabled={busy !== null || !pasteInput.trim()}
        >
          {busy === 'discover' ? (
            <LoaderCircle className="mr-1.5 size-3.5 animate-spin" />
          ) : null}
          Discover
        </Button>
      </div>
    </div>
  )
}
