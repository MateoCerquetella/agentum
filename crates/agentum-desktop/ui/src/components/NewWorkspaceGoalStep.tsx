import React, { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowRight } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DialogHeader, DialogTitle } from '@/components/ui/dialog'
import RepoCombobox from '@/components/repo/RepoCombobox'
import {
  OPTIONAL_WORKSPACE_STEPS,
  firstGoalStepBlocker,
  isGoalStepReady
} from '@/lib/workspace-goal-step'
import type { Repo } from '../../../shared/types'

// Spec 008 F3 (AC 9–11): the goal-first entry that FRONTS the existing composer
// (D-C / D3). This component is intentionally thin — it owns only its local
// goal/workdir state and hands captured values up via `onContinue`/`onSkip`; the
// composer (`useComposerState`) stays the untouched creation engine, reached
// after "Continue" (seeded) or "Skip to details" (no goal framing). All the pure
// decisions live in `lib/workspace-goal-step.ts` so they are unit-tested without
// a DOM.

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

  // Why: the modal's Dialog focuses the repo combobox on open, but on the goal
  // step that combobox isn't mounted yet — so put the caret in the goal box, the
  // first thing to capture (AC 9: the goal input is the first step).
  useEffect(() => {
    goalRef.current?.focus()
  }, [])

  const ready = isGoalStepReady({ goal, repoId })
  const blocker = firstGoalStepBlocker({ goal, repoId })

  const handleContinue = useCallback(() => {
    if (!isGoalStepReady({ goal, repoId })) {
      return
    }
    onContinue(goal, repoId)
  }, [goal, onContinue, repoId])

  const handleGoalKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // Cmd/Ctrl+Enter advances (parity with the composer's submit shortcut);
      // a bare Enter keeps its native newline so multi-line goals are natural.
      if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
        event.preventDefault()
        handleContinue()
      }
    },
    [handleContinue]
  )

  return (
    <>
      <DialogHeader className="gap-1">
        <DialogTitle className="text-base font-semibold">{primaryLabel}</DialogTitle>
        <p className="text-xs text-muted-foreground">
          Describe what you want to build. You can create the worktree, scaffold a spec, and file a
          tracker issue next — all optional.
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

        {/* Pre-offer the three optional steps so goal-first makes the pipeline
            (worktree → spec → tracker) visible before the composer opens. */}
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
          {!ready && blocker ? (
            <span className="text-[11px] text-muted-foreground">{blocker}</span>
          ) : null}
          <Button type="button" size="sm" onClick={handleContinue} disabled={!ready}>
            Continue
            <ArrowRight className="size-3.5" />
          </Button>
        </div>
      </div>
    </>
  )
}
