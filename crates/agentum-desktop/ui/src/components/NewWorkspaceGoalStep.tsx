import React, { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowRight, FolderOpen, LoaderCircle } from 'lucide-react'
import { api } from '@/tauri'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { DialogHeader, DialogTitle } from '@/components/ui/dialog'
import RepoCombobox from '@/components/repo/RepoCombobox'
import {
  OPTIONAL_WORKSPACE_STEPS,
  firstGoalStepBlocker,
  isGoalStepReady
} from '@/lib/workspace-goal-step'
import {
  DEFAULT_TEMPLATE_REPO,
  deriveTemplateRepoName,
  firstTemplateModeBlocker,
  isTemplateModeReady
} from '@/lib/workspace-provision-step'
import { createRepoFromTemplate } from '@/runtime/github-projects-client'
import type { Repo } from '../../../shared/types'

// Spec 008 F3 (AC 9–11): the goal-first entry that FRONTS the existing composer
// (D-C / D3). This component is intentionally thin — it owns only its local
// goal/workdir state and hands captured values up via `onContinue`/`onSkip`; the
// composer (`useComposerState`) stays the untouched creation engine, reached
// after "Continue" (seeded) or "Skip to details" (no goal framing). All the pure
// decisions live in `lib/workspace-goal-step.ts` so they are unit-tested without
// a DOM.
//
// Spec 010 F3 (AC 9): a workdir-target mode toggle — "Existing project" (the
// byte-identical combobox path) | "New repo from template". Template mode
// produces the registered repoId INSIDE its Continue (create → clone → register
// through the SAME store action the add-repo dialog's submit uses) and only
// then calls `onContinue(goal, newRepoId)` — so `isGoalStepReady` /
// `GoalStepInputs` stay untouched (§7.6).

export default function NewWorkspaceGoalStep({
  repos,
  initialRepoId,
  initialGoal,
  primaryLabel = 'Create Workspace',
  onContinue,
  onSkip
}: {
  /** Eligible workdir targets — the composer's repo list (repos with a path). */
  repos: Repo[]
  /** Pre-selected workdir (e.g. sidebar "+" on a project). Optional (AC 9: no
   *  repo is required BEFORE the goal is captured). */
  initialRepoId?: string
  initialGoal?: string
  primaryLabel?: string
  /** "Continue": reveal the seeded composer (goal + the chosen workdir target). */
  onContinue: (goal: string, repoId: string) => void
  /** "Skip to details" (D3): reveal the mechanics-first composer, no goal seed. */
  onSkip: () => void
}): React.JSX.Element {
  const [goal, setGoal] = useState(initialGoal ?? '')
  const [repoId, setRepoId] = useState(initialRepoId ?? '')
  const goalRef = useRef<HTMLTextAreaElement | null>(null)

  // Spec 010 F3: template-mode local state. The repo NAME live-seeds from the
  // goal (deriveTemplateRepoName) until the user hand-edits it; the template
  // defaults to the D4 constant and stays editable.
  const addRepoPath = useAppStore((s) => s.addRepoPath)
  const [mode, setMode] = useState<'existing' | 'template'>('existing')
  const [owner, setOwner] = useState('')
  const [repoName, setRepoName] = useState('')
  const [repoNameTouched, setRepoNameTouched] = useState(false)
  const [templateRepo, setTemplateRepo] = useState(DEFAULT_TEMPLATE_REPO)
  const [directory, setDirectory] = useState('')
  const [visibility, setVisibility] = useState<'private' | 'public'>('private')
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)

  // Why: the modal's Dialog focuses the repo combobox on open, but on the goal
  // step that combobox isn't mounted yet — so put the caret in the goal box, the
  // first thing to capture (AC 9: the goal input is the first step).
  useEffect(() => {
    goalRef.current?.focus()
  }, [])

  const effectiveRepoName = repoNameTouched ? repoName : deriveTemplateRepoName(goal)

  const ready =
    mode === 'template'
      ? isTemplateModeReady({ goal, owner, name: effectiveRepoName, templateRepo, directory })
      : isGoalStepReady({ goal, repoId })
  const blocker =
    mode === 'template'
      ? firstTemplateModeBlocker({ goal, owner, name: effectiveRepoName, templateRepo, directory })
      : firstGoalStepBlocker({ goal, repoId })

  const handleContinue = useCallback(() => {
    if (!isGoalStepReady({ goal, repoId })) {
      return
    }
    onContinue(goal, repoId)
  }, [goal, onContinue, repoId])

  // Spec 010 F3 (§7.6): template-mode Continue creates + clones the repo,
  // registers the clone through the SAME store action the add-repo dialog's
  // submit uses (`addRepoPath` — no parallel registration path), and only then
  // continues with the fresh repoId. Failures render inline — never silent.
  const handleTemplateContinue = useCallback(async () => {
    const inputs = { goal, owner, name: effectiveRepoName, templateRepo, directory }
    if (!isTemplateModeReady(inputs) || creating) {
      return
    }
    setCreating(true)
    setCreateError(null)
    try {
      const result = await createRepoFromTemplate({
        owner: owner.trim(),
        name: effectiveRepoName.trim(),
        templateRepo: templateRepo.trim(),
        directory: directory.trim(),
        visibility
      })
      const repo = await addRepoPath(result.path)
      if (!repo) {
        // addRepoPath already toasted its own error; still say so inline.
        setCreateError(
          `The repo was created at ${result.path} but could not be registered — add it via "Add project".`
        )
        return
      }
      onContinue(goal, repo.id)
    } catch (err) {
      // gh's stderr rides the thrown message verbatim (e.g. "<template> is not
      // a template repository") — render it unedited.
      setCreateError(err instanceof Error ? err.message : String(err))
    } finally {
      setCreating(false)
    }
  }, [
    addRepoPath,
    creating,
    directory,
    effectiveRepoName,
    goal,
    onContinue,
    owner,
    templateRepo,
    visibility
  ])

  const advance = useCallback(() => {
    if (mode === 'template') {
      void handleTemplateContinue()
    } else {
      handleContinue()
    }
  }, [handleContinue, handleTemplateContinue, mode])

  const handleGoalKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // Cmd/Ctrl+Enter advances (parity with the composer's submit shortcut);
      // a bare Enter keeps its native newline so multi-line goals are natural.
      if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
        event.preventDefault()
        advance()
      }
    },
    [advance]
  )

  const handleBrowseDirectory = useCallback(async () => {
    try {
      const picked = (await api.repos.pickFolder()) as string | null
      if (picked) {
        setDirectory(picked)
      }
    } catch {
      // The OS picker being unavailable is not an error state; the field stays
      // hand-editable.
    }
  }, [])

  // The compact field styling the binding editor established for form inputs.
  const fieldClassName = 'h-8 text-xs'

  return (
    <>
      <DialogHeader className="gap-1">
        <DialogTitle className="text-base font-semibold">{primaryLabel}</DialogTitle>
        <p className="text-xs text-muted-foreground">
          Describe what you want to build. You can create the worktree, scaffold a spec, file a
          tracker issue, and provision the repo next — all optional.
        </p>
      </DialogHeader>

      <div className="space-y-4">
        <div className="space-y-1">
          <label htmlFor="workspace-goal" className="text-xs font-medium text-muted-foreground">
            Goal
          </label>
          <textarea
            id="workspace-goal"
            ref={goalRef}
            value={goal}
            onChange={(event) => setGoal(event.target.value)}
            onKeyDown={handleGoalKeyDown}
            placeholder="e.g. Add a dark-mode toggle to Settings that persists per user"
            rows={4}
            className="w-full min-w-0 resize-none rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs transition-[color,box-shadow] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
          />
        </div>

        {/* Spec 010 F3: the workdir-target mode toggle. "Existing project" is
            today's path, byte-identical; template mode is born-ready's entry. */}
        <div className="flex items-center gap-1 rounded-md border border-border/70 bg-muted/30 p-0.5">
          {(
            [
              ['existing', 'Existing project'],
              ['template', 'New repo from template']
            ] as const
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              onClick={() => setMode(value)}
              className={`flex-1 rounded px-2 py-1 text-[11px] font-medium transition-colors ${
                mode === value
                  ? 'bg-background text-foreground shadow-xs'
                  : 'text-muted-foreground hover:text-foreground'
              }`}
            >
              {label}
            </button>
          ))}
        </div>
        {/* #280: template mode needs a plain-words explanation — it is the
            uncommon path and read as noise without one. */}
        {mode === 'template' ? (
          <p className="text-[11px] text-muted-foreground">
            Creates a brand-new GitHub repository from a starter template (the SDD skeleton),
            clones it into the folder you pick, and uses it as this workspace's project. Working
            on a repo you already have? Choose "Existing project".
          </p>
        ) : null}

        {mode === 'existing' ? (
          <div className="space-y-1">
            <label className="text-xs font-medium text-muted-foreground">Project (workdir)</label>
            <RepoCombobox
              repos={repos}
              value={repoId}
              onValueChange={setRepoId}
              placeholder="Choose project"
              triggerClassName="h-9 w-full border-input text-sm focus:border-ring focus:ring-[3px] focus:ring-ring/50"
              showStandaloneAddButton={false}
            />
            <p className="text-[11px] text-muted-foreground">
              A workspace needs a workdir. Skip fresh-worktree creation later to use an existing
              folder/branch as-is.
            </p>
          </div>
        ) : (
          <div className="space-y-2">
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">Owner</label>
                <Input
                  value={owner}
                  onChange={(e) => setOwner(e.target.value)}
                  placeholder="your-login or org"
                  className={fieldClassName}
                  disabled={creating}
                />
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">Repository name</label>
                <Input
                  value={effectiveRepoName}
                  onChange={(e) => {
                    setRepoNameTouched(true)
                    setRepoName(e.target.value)
                  }}
                  placeholder="seeded from your goal"
                  className={fieldClassName}
                  disabled={creating}
                />
              </div>
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium text-muted-foreground">Template</label>
              <Input
                value={templateRepo}
                onChange={(e) => setTemplateRepo(e.target.value)}
                placeholder={DEFAULT_TEMPLATE_REPO}
                className={fieldClassName}
                disabled={creating}
              />
            </div>
            <div className="space-y-1">
              <label className="text-xs font-medium text-muted-foreground">Clone into</label>
              <div className="flex items-center gap-2">
                <Input
                  value={directory}
                  onChange={(e) => setDirectory(e.target.value)}
                  placeholder="~/projects"
                  className={fieldClassName}
                  disabled={creating}
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-8 shrink-0"
                  onClick={() => void handleBrowseDirectory()}
                  disabled={creating}
                >
                  <FolderOpen className="size-3.5" />
                  Browse
                </Button>
              </div>
            </div>
            <div className="flex items-center gap-3">
              <span className="text-xs font-medium text-muted-foreground">Visibility</span>
              {(['private', 'public'] as const).map((value) => (
                <label key={value} className="flex items-center gap-1 text-xs text-muted-foreground">
                  <input
                    type="radio"
                    name="template-repo-visibility"
                    checked={visibility === value}
                    onChange={() => setVisibility(value)}
                    disabled={creating}
                  />
                  {value}
                </label>
              ))}
            </div>
            {createError ? <p className="text-xs text-destructive">{createError}</p> : null}
          </div>
        )}

        {/* Pre-offer the optional steps so goal-first makes the pipeline
            (worktree → spec → tracker → provision) visible before the composer
            opens. */}
        <div className="rounded-md border border-border/70 bg-muted/30 px-3 py-2">
          <p className="text-[11px] font-medium text-muted-foreground">Next, optionally:</p>
          <ul className="mt-1 space-y-0.5">
            {OPTIONAL_WORKSPACE_STEPS.map((step) => (
              <li key={step.id} className="text-[11px] text-muted-foreground">
                • {step.label}
              </li>
            ))}
          </ul>
        </div>
      </div>

      <div className="mt-1 flex items-center justify-between gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={onSkip}>
          Skip to details
        </Button>
        <div className="flex items-center gap-2">
          {!ready && blocker && !creating ? (
            <span className="text-[11px] text-muted-foreground">{blocker}</span>
          ) : null}
          <Button type="button" size="sm" onClick={advance} disabled={!ready || creating}>
            {creating ? (
              <>
                <LoaderCircle className="size-3.5 animate-spin" />
                Creating repo…
              </>
            ) : (
              <>
                Continue
                <ArrowRight className="size-3.5" />
              </>
            )}
          </Button>
        </div>
      </div>
    </>
  )
}
