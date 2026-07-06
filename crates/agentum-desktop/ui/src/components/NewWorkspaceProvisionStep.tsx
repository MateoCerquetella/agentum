import React, { useCallback, useEffect, useState } from 'react'
import { ArrowRight, LoaderCircle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { ProjectBindingEditor } from '@/components/github-projects/ProjectBindingEditor'
import {
  provisionCommitFileList,
  summarizeProvisionReport,
  type ProvisionReport
} from '@/lib/workspace-provision-step'
import {
  getProjectBinding,
  provisionWorkspace,
  type ProvisionProjectChoice
} from '@/runtime/github-projects-client'

// Spec 010 F3: the modal-level `'provision'` phase between goal and details —
// a workspace is BORN READY. Mounts the SHARED ProjectBindingEditor (D7's
// second mount: link an existing Projects v2 board with the full mapping
// editor) or a create-a-board form (D5), plus the D8 consent checklist: the
// commit toggle is default ON, explicitly declinable, and lists the EXACT five
// contract paths (`provisionCommitFileList()`) landing on the project's
// current branch. "Provision & continue" runs the one idempotent server
// ensure and renders the per-step report inline — failures are warnings,
// never blockers; "Skip" goes straight to details.

export default function NewWorkspaceProvisionStep({
  workdir,
  onContinue,
  onSkip
}: {
  /** The chosen project's root path (the goal step's workdir target). */
  workdir: string
  onContinue: () => void
  onSkip: () => void
}): React.JSX.Element {
  const [slug, setSlug] = useState<string | null>(null)
  const [slugError, setSlugError] = useState<string | null>(null)
  const [bound, setBound] = useState(false)
  const [boardMode, setBoardMode] = useState<'link' | 'create'>('link')
  const [createOwner, setCreateOwner] = useState('')
  const [createOwnerType, setCreateOwnerType] = useState<'user' | 'organization'>('user')
  const [createTitle, setCreateTitle] = useState('')
  // D8: default ON, explicitly visible and declinable.
  const [commitScaffold, setCommitScaffold] = useState(true)
  const [running, setRunning] = useState(false)
  const [report, setReport] = useState<ProvisionReport | null>(null)
  const [error, setError] = useState<string | null>(null)

  // Resolve the slug once: it prefills the create-board owner/title and tells
  // us whether a binding already exists (then create/link is a server no-op —
  // "already bound"). A repo with no GitHub origin can't provision (labels and
  // boards key off the slug) — say so, never a dead end: Skip stays available.
  useEffect(() => {
    let cancelled = false
    setSlug(null)
    setSlugError(null)
    setBound(false)
    void getProjectBinding({ workdir })
      .then((res) => {
        if (cancelled) return
        setSlug(res.slug)
        setBound(res.binding !== null)
        const [owner, name] = res.slug.split('/')
        setCreateOwner((prev) => (prev.trim().length > 0 ? prev : (owner ?? '')))
        setCreateTitle((prev) => (prev.trim().length > 0 ? prev : (name ?? '')))
      })
      .catch((err: unknown) => {
        if (cancelled) return
        setSlugError(err instanceof Error ? err.message : String(err))
      })
    return () => {
      cancelled = true
    }
  }, [workdir])

  const handleProvision = useCallback(async () => {
    setRunning(true)
    setError(null)
    try {
      // Link mode rides the editor's own bind (the binding already exists
      // server-side → the ensure reports "already bound"); only an unbound
      // create-mode run ships a `project` for the server to create+bind.
      const project: ProvisionProjectChoice | undefined =
        !bound && boardMode === 'create' && createOwner.trim() && createTitle.trim()
          ? {
              create: true,
              owner: createOwner.trim(),
              ownerType: createOwnerType,
              title: createTitle.trim()
            }
          : undefined
      const res = await provisionWorkspace({
        workdir,
        ...(project ? { project } : {}),
        commitScaffold
      })
      setReport(res)
    } catch (err) {
      // A hard failure (request shape / unreachable) is still only a warning
      // here — Continue and Skip stay available (D8: never a blocker).
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRunning(false)
    }
  }, [boardMode, bound, commitScaffold, createOwner, createOwnerType, createTitle, workdir])

  const commitFiles = provisionCommitFileList()
  const summary = report ? summarizeProvisionReport(report) : null

  return (
    <>
      <DialogHeader className="gap-1">
        <DialogTitle className="text-base font-semibold">Provision repository</DialogTitle>
        <p className="text-xs text-muted-foreground">
          Make {slug ?? 'this repo'} ready for gated runs: status labels, a Projects v2 board, and
          the .agentum-harness scaffold. Every step is optional and re-running changes nothing.
        </p>
      </DialogHeader>

      <div className="max-h-[60vh] space-y-4 overflow-y-auto pr-1">
        {slugError ? (
          <p className="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
            No GitHub repository resolved for this project — provisioning needs a GitHub origin.
            You can skip this step and add one later.
          </p>
        ) : null}

        <div className="space-y-2">
          <p className="text-xs font-medium text-muted-foreground">Projects v2 board</p>
          {!bound ? (
            <div className="flex items-center gap-1 rounded-md border border-border/70 bg-muted/30 p-0.5">
              {(
                [
                  ['link', 'Link existing board'],
                  ['create', 'Create a new board']
                ] as const
              ).map(([value, label]) => (
                <button
                  key={value}
                  type="button"
                  onClick={() => setBoardMode(value)}
                  className={`flex-1 rounded px-2 py-1 text-[11px] font-medium transition-colors ${
                    boardMode === value
                      ? 'bg-background text-foreground shadow-xs'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                >
                  {label}
                </button>
              ))}
            </div>
          ) : null}
          {bound || boardMode === 'link' ? (
            // D7's second mount: the SAME editor Settings → Integrations uses
            // (pick → discover → per-phase selects with fallback hints → Save).
            <ProjectBindingEditor workdir={workdir} onBound={() => setBound(true)} />
          ) : (
            <div className="space-y-2">
              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <label className="text-[11px] font-medium text-muted-foreground">Owner</label>
                  <Input
                    value={createOwner}
                    onChange={(e) => setCreateOwner(e.target.value)}
                    placeholder="login or org"
                    className="h-8 text-xs"
                    disabled={running}
                  />
                </div>
                <div className="space-y-1">
                  <label className="text-[11px] font-medium text-muted-foreground">Owner type</label>
                  <select
                    value={createOwnerType}
                    onChange={(e) =>
                      setCreateOwnerType(e.target.value === 'organization' ? 'organization' : 'user')
                    }
                    disabled={running}
                    className="h-8 w-full min-w-0 rounded-md border border-input bg-background px-2 text-xs text-foreground"
                  >
                    <option value="user">user</option>
                    <option value="organization">organization</option>
                  </select>
                </div>
              </div>
              <div className="space-y-1">
                <label className="text-[11px] font-medium text-muted-foreground">Board title</label>
                <Input
                  value={createTitle}
                  onChange={(e) => setCreateTitle(e.target.value)}
                  placeholder="e.g. the repo name"
                  className="h-8 text-xs"
                  disabled={running}
                />
              </div>
              <p className="text-[11px] text-muted-foreground">
                A new board carries GitHub's default Todo / In Progress / Done columns; Ready to
                Test and Blocked fall back to In Progress until you add columns and re-discover
                (Settings → Integrations).
              </p>
            </div>
          )}
        </div>

        <div className="space-y-2 rounded-md border border-border/70 bg-muted/30 px-3 py-2">
          <p className="text-[11px] font-medium text-muted-foreground">Provisioning will:</p>
          <ul className="space-y-0.5 text-[11px] text-muted-foreground">
            <li>• Ensure the five status/* labels on {slug ?? 'the repo'}</li>
            <li>• {bound ? 'Keep the existing board binding' : 'Link or create the board and bind it'}</li>
            <li>• Scaffold .agentum-harness/ (existing files kept)</li>
          </ul>
          {/* D8: the commit consent — default ON, explicitly declinable, naming
              the target branch and the EXACT committed paths. */}
          <label className="flex items-center justify-between gap-3 pt-1">
            <span className="text-[11px] text-muted-foreground">
              Commit and push the scaffold contract files to the project's current branch (plain
              push, never force)
            </span>
            <button
              role="switch"
              aria-checked={commitScaffold}
              onClick={() => setCommitScaffold((v) => !v)}
              disabled={running}
              className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors ${
                commitScaffold ? 'bg-foreground' : 'bg-muted-foreground/30'
              }`}
            >
              <span
                className={`inline-block h-3.5 w-3.5 transform rounded-full bg-background shadow-sm transition-transform ${
                  commitScaffold ? 'translate-x-4' : 'translate-x-0.5'
                }`}
              />
            </button>
          </label>
          {commitScaffold ? (
            <ul className="space-y-0.5 pl-3 text-[11px] text-muted-foreground/80">
              {commitFiles.map((file) => (
                <li key={file} className="font-mono">
                  {file}
                </li>
              ))}
            </ul>
          ) : null}
        </div>

        {error ? <p className="text-xs text-destructive">{error}</p> : null}

        {summary ? (
          <div className="space-y-1 rounded-md border border-border/70 px-3 py-2">
            <p className="text-[11px] font-medium text-muted-foreground">Provision report</p>
            {summary.map((line) => (
              <p
                key={line.id}
                className={`text-[11px] ${
                  line.ok ? 'text-muted-foreground' : 'text-amber-700 dark:text-amber-300'
                }`}
              >
                <span className="font-medium">{line.label}:</span> {line.text}
                {!line.ok ? ' (warning — creation continues)' : ''}
              </p>
            ))}
          </div>
        ) : null}
      </div>

      <div className="mt-1 flex items-center justify-between gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={onSkip} disabled={running}>
          Skip
        </Button>
        {report ? (
          <Button type="button" size="sm" onClick={onContinue}>
            Continue
            <ArrowRight className="size-3.5" />
          </Button>
        ) : (
          <Button
            type="button"
            size="sm"
            onClick={() => void handleProvision()}
            disabled={running || slugError !== null || slug === null}
          >
            {running ? (
              <>
                <LoaderCircle className="size-3.5 animate-spin" />
                Provisioning…
              </>
            ) : (
              <>
                Provision & continue
                <ArrowRight className="size-3.5" />
              </>
            )}
          </Button>
        )}
      </div>
    </>
  )
}
