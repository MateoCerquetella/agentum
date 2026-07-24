import React from 'react'
import { LoaderCircle, Plus, Sparkles } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { SmartWorkspaceNameSelection } from '@/components/new-workspace/SmartWorkspaceNameField'

export type NewWorkspaceAutomationPanelProps = {
  primaryActionLabel: string
  selectedRepoIsGit: boolean
  selectedSource: SmartWorkspaceNameSelection | null
  canCreateGithubIssue: boolean
  createIssueOpen: boolean
  onCreateIssueOpenChange: (open: boolean) => void
  createIssueTitle: string
  onCreateIssueTitleChange: (value: string) => void
  createIssueBody: string
  onCreateIssueBodyChange: (value: string) => void
  createIssueSubmitting: boolean
  createIssueError: string | null
  onCreateIssueSubmit: () => void
  createIssueGenerating: boolean
  onGenerateIssueBody: () => void
  createIssueLabels: string[]
  createIssueLabelOptions: string[] | null
  onToggleCreateIssueLabel: (label: string) => void
  canScaffoldSpec: boolean
  scaffoldSpec: boolean
  onScaffoldSpecChange: (value: boolean) => void
  canStartGatedRun: boolean
  startGatedRun: boolean
  onStartGatedRunChange: (value: boolean) => void
  sddRolesEnabled: boolean
}

function WorkflowStep({
  number,
  title,
  children
}: {
  number: number
  title: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <li className="relative grid grid-cols-[1.5rem_minmax(0,1fr)] gap-2.5 pb-4 last:pb-0">
      <span className="relative z-10 flex size-6 items-center justify-center rounded-full border border-border bg-background text-[10px] font-semibold text-muted-foreground">
        {number}
      </span>
      <div className="min-w-0 space-y-1">
        <div className="text-xs font-semibold text-foreground">{title}</div>
        {children}
      </div>
    </li>
  )
}

export default function NewWorkspaceAutomationPanel({
  primaryActionLabel,
  selectedRepoIsGit,
  selectedSource,
  canCreateGithubIssue,
  createIssueOpen,
  onCreateIssueOpenChange,
  createIssueTitle,
  onCreateIssueTitleChange,
  createIssueBody,
  onCreateIssueBodyChange,
  createIssueSubmitting,
  createIssueError,
  onCreateIssueSubmit,
  createIssueGenerating,
  onGenerateIssueBody,
  createIssueLabels,
  createIssueLabelOptions,
  onToggleCreateIssueLabel,
  canScaffoldSpec,
  scaffoldSpec,
  onScaffoldSpecChange,
  canStartGatedRun,
  startGatedRun,
  onStartGatedRunChange,
  sddRolesEnabled
}: NewWorkspaceAutomationPanelProps): React.JSX.Element {
  const specIncludedByRun = canScaffoldSpec && startGatedRun
  const linkedIssueSource =
    selectedSource &&
    (selectedSource.kind === 'github-issue' ||
      selectedSource.kind === 'gitlab-issue' ||
      selectedSource.kind === 'linear')
      ? selectedSource
      : null

  return (
    <aside
      aria-label="Issue, worktree, spec, and run options"
      className="mt-3 min-w-0 rounded-lg border border-border/70 bg-muted/20 p-4 md:sticky md:top-0 md:self-start"
    >
      <div className="border-b border-border/60 pb-3">
        <div className="text-xs font-semibold text-foreground">Issue → Worktree → Spec → Run</div>
        <p className="mt-1 text-[11px] leading-4 text-muted-foreground">
          Review the whole flow here. Issue, spec, and run are optional; Cancel below leaves without
          creating anything.
        </p>
      </div>

      <ol className="relative mt-4 before:absolute before:top-3 before:bottom-3 before:left-[0.6875rem] before:w-px before:bg-border">
        <WorkflowStep number={1} title="Issue">
          {linkedIssueSource ? (
            <div className="rounded-md border border-border/70 bg-background/70 px-2.5 py-2">
              <div className="text-[11px] font-medium text-foreground">
                {linkedIssueSource.label}
              </div>
              <div className="mt-0.5 text-[10px] text-muted-foreground">
                Linked from Create From
              </div>
            </div>
          ) : canCreateGithubIssue && !createIssueOpen ? (
            <>
              <p className="text-[11px] leading-4 text-muted-foreground">
                Link one with Create From, or file a new GitHub issue.
              </p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => onCreateIssueOpenChange(true)}
                className="mt-1 h-7 text-[11px]"
              >
                <Plus className="size-3.5" />
                Create GitHub issue
              </Button>
            </>
          ) : !createIssueOpen ? (
            <p className="text-[11px] leading-4 text-muted-foreground">
              {selectedSource
                ? 'Optional. The current Create From source is not an issue.'
                : 'Optional. Choose an issue in Create From to enable spec and run.'}
            </p>
          ) : null}

          {canCreateGithubIssue && createIssueOpen ? (
            <div className="mt-2 space-y-2 rounded-md border border-border/70 bg-background/70 p-3">
              <div className="text-[11px] font-medium text-foreground">New GitHub issue</div>
              <input
                type="text"
                value={createIssueTitle}
                onChange={(event) => onCreateIssueTitleChange(event.target.value)}
                placeholder="Issue title"
                disabled={createIssueSubmitting}
                className="w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1.5 text-xs shadow-xs transition-[color,box-shadow] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
              />
              <textarea
                value={createIssueBody}
                onChange={(event) => onCreateIssueBodyChange(event.target.value)}
                placeholder="Body (optional)"
                rows={3}
                disabled={createIssueSubmitting || createIssueGenerating}
                className="w-full min-w-0 resize-none rounded-md border border-input bg-transparent px-3 py-1.5 text-xs shadow-xs transition-[color,box-shadow] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
              />
              {!createIssueBody.trim() ? (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={onGenerateIssueBody}
                  disabled={
                    createIssueSubmitting || createIssueGenerating || !createIssueTitle.trim()
                  }
                  className="-ml-2 h-7 text-[11px] text-muted-foreground hover:text-foreground"
                >
                  {createIssueGenerating ? (
                    <LoaderCircle className="size-3.5 animate-spin" />
                  ) : (
                    <Sparkles className="size-3.5" />
                  )}
                  {createIssueGenerating ? 'Generating…' : 'Generate description'}
                </Button>
              ) : null}
              {createIssueLabelOptions && createIssueLabelOptions.length > 0 ? (
                <div className="flex flex-wrap gap-1.5">
                  {createIssueLabelOptions.map((label) => {
                    const selected = createIssueLabels.includes(label)
                    return (
                      <button
                        key={label}
                        type="button"
                        onClick={() => onToggleCreateIssueLabel(label)}
                        disabled={createIssueSubmitting}
                        aria-pressed={selected}
                        className={cn(
                          'rounded-full border px-2 py-1 text-[10px] leading-none transition',
                          selected
                            ? 'border-foreground/60 bg-foreground text-background'
                            : 'border-border/70 bg-muted/40 text-muted-foreground hover:text-foreground'
                        )}
                      >
                        {label}
                      </button>
                    )
                  })}
                </div>
              ) : null}
              {createIssueError ? (
                <div role="alert" className="text-[11px] text-destructive">
                  {createIssueError}
                </div>
              ) : null}
              <div className="flex items-center justify-end gap-2">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => onCreateIssueOpenChange(false)}
                  disabled={createIssueSubmitting}
                  className="h-7 text-[11px]"
                >
                  Cancel
                </Button>
                <Button
                  type="button"
                  size="sm"
                  onClick={onCreateIssueSubmit}
                  disabled={
                    createIssueSubmitting || createIssueGenerating || !createIssueTitle.trim()
                  }
                  className="h-7 text-[11px]"
                >
                  {createIssueSubmitting ? (
                    <LoaderCircle className="size-3.5 animate-spin" />
                  ) : null}
                  Create issue
                </Button>
              </div>
            </div>
          ) : null}
        </WorkflowStep>

        <WorkflowStep number={2} title="Worktree">
          <p className="text-[11px] leading-4 text-muted-foreground">
            {selectedRepoIsGit
              ? `Created when you choose ${primaryActionLabel}.`
              : 'The selected folder is used as the workspace without creating a Git worktree.'}
          </p>
        </WorkflowStep>

        <WorkflowStep number={3} title="Spec">
          <label
            className={cn(
              'flex items-start gap-2',
              canScaffoldSpec && !startGatedRun ? 'cursor-pointer' : 'cursor-not-allowed opacity-70'
            )}
          >
            <input
              type="checkbox"
              aria-label="Scaffold spec from issue"
              checked={specIncludedByRun || scaffoldSpec}
              disabled={!canScaffoldSpec || startGatedRun}
              onChange={(event) => onScaffoldSpecChange(event.target.checked)}
              className="mt-0.5 size-4 shrink-0 accent-foreground"
            />
            <span className="text-[11px] leading-4 text-muted-foreground">
              {specIncludedByRun
                ? 'Included in the gated run.'
                : canScaffoldSpec
                  ? 'Scaffold a harness spec and backlog from the linked issue.'
                  : 'Optional. Link a supported GitHub issue to enable it.'}
            </span>
          </label>
        </WorkflowStep>

        <WorkflowStep number={4} title="Run">
          <label
            className={cn(
              'flex items-start gap-2',
              canStartGatedRun ? 'cursor-pointer' : 'cursor-not-allowed opacity-70'
            )}
          >
            <input
              type="checkbox"
              aria-label="Start gated run"
              checked={startGatedRun}
              disabled={!canStartGatedRun}
              onChange={(event) => onStartGatedRunChange(event.target.checked)}
              className="mt-0.5 size-4 shrink-0 accent-foreground"
            />
            <span className="text-[11px] leading-4 text-muted-foreground">
              {startGatedRun
                ? sddRolesEnabled
                  ? 'Start the PM → Architect → Build → Review gated role loop.'
                  : 'Start verification-gated agents from the linked issue.'
                : canStartGatedRun
                  ? 'Optionally plan and run the linked issue with gated agents.'
                  : 'Optional. Link a supported GitHub issue to enable it.'}
            </span>
          </label>
        </WorkflowStep>
      </ol>
    </aside>
  )
}
