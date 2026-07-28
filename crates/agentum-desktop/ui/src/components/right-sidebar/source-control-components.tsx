import { api } from '@/tauri'
import { appendCommitFailureCustomInstruction, getCommitFailureKindLabel } from './source-control-prompts'
import { hostedReviewCreationCopy } from './hosted-review-display'
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ArrowDownUp, ArrowUp, ChevronDown, CloudUpload, Minus, Plus, RefreshCw, Settings2, Sparkle, Sparkles, Square, PencilLine, Undo2, Check, Copy, Folder, FolderOpen, GitMerge, GitPullRequestArrow, List, ListTree, MessageSquare, Trash, Trash2, TriangleAlert, type LucideIcon } from 'lucide-react'
import { basename, dirname, joinPath } from '@/lib/path'
import { cn } from '@/lib/utils'
import { WORKSPACE_FILE_PATH_MIME } from '@/lib/workspace-file-drag'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { type PrimaryAction, type RemoteOpKind } from './source-control-primary-action'
import { type DropdownActionKind, type DropdownEntry } from './source-control-dropdown-items'
import { type DiscardAllArea } from './discard-all-sequence'
import { getFileTypeIcon } from '@/lib/file-type-icons'
import { type SourceControlTreeNode } from './source-control-tree'
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from '@/components/ui/context-menu'
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { formatDiffComment } from '@/lib/diff-comments-format'
import { getDiffCommentLineLabel, getDiffCommentSource } from '@/lib/diff-comment-compat'
import { focusTerminalTabSurface } from '@/lib/focus-terminal-tab-surface'
import { QuickLaunchAgentMenuItems } from '@/components/tab-bar/QuickLaunchButton'
import { stripBaseRef } from './useCreatePullRequestDialogFields'
import { type GitHistoryPanelState } from './GitHistoryPanel'
import type { DiffComment, GitBranchChangeEntry, GitBranchCompareSummary, GitConflictOperation, GitStatusEntry, SourceControlViewMode } from '@/shared/types'
import type { HostedReviewProvider } from '@/shared/hosted-review'
import { STATUS_COLORS, STATUS_LABELS } from './status-display'
import type { SourceControlAiOperation } from '@/shared/source-control-ai-types'
import { getCommitFailureDialogWorktreeKey, shouldShowCommitFailureDialog, syncCommitFailureDialogState, type CommitFailureDialogState } from './commit-failure-dialog-state'
import { hasExpandedCommitFailureDetails, summarizeCommitFailure } from './commit-failure-summary'

type SourceControlAiInstructionGuidance = {
  operation: SourceControlAiOperation
  repoBacked: boolean
  onOpenSettings: () => void
}

// Why: directional signifiers ahead of each primary action label. Commit
// (✓) is affirmative; Push (↑) points in the direction data flows; Sync
// (↕) is bidirectional; Publish gets a cloud-up to distinguish the
// first-time publish from a subsequent push. Pull is intentionally
// icon-less — the down-arrow read as a download/save affordance and was
// removed. Keeping the mapping outside the render function avoids
// reallocating it on every render.
const PRIMARY_ICONS: Partial<
  Record<
    PrimaryAction['kind'],
    React.ComponentType<{ className?: string; 'aria-hidden'?: boolean | 'true' | 'false' }>
  >
> = {
  commit: Check,
  stage: Plus,
  push: ArrowUp,
  sync: ArrowDownUp,
  publish: CloudUpload,
  create_pr: GitPullRequestArrow
}

// Why: unstaged ("Changes") is listed first so that conflict files — which
// are assigned area:'unstaged' by the parser — appear above "Staged Changes".
// This keeps unresolved conflicts visible at the top of the list where the
// user won't miss them.
export const SECTION_ORDER = ['unstaged', 'staged', 'untracked'] as const

export const SECTION_LABELS: Record<(typeof SECTION_ORDER)[number], string> = {
  staged: 'Staged Changes',
  unstaged: 'Changes',
  untracked: 'Untracked Files'
}

// Why: row action buttons host Radix Tooltip triggers. Keeping the overlay
// measurable prevents transient top-left tooltip placement during hover.
export const SOURCE_CONTROL_ROW_ACTION_OVERLAY_CLASS =
  'absolute right-0 top-0 bottom-0 flex shrink-0 items-center gap-1.5 bg-accent pr-3 pl-2 opacity-0 pointer-events-none transition-opacity group-hover:opacity-100 group-hover:pointer-events-auto focus-within:opacity-100 focus-within:pointer-events-auto [@media(hover:none)]:opacity-100 [@media(hover:none)]:pointer-events-auto'

export const SOURCE_CONTROL_TREE_INDENT_PX = 12

export const SOURCE_CONTROL_TREE_DIRECTORY_PADDING_PX = 8

export const SOURCE_CONTROL_TREE_FILE_PADDING_PX = 20

export const EMPTY_GIT_HISTORY_STATE: GitHistoryPanelState = { status: 'idle' }

export function useCopyFeedbackState<T>(resetValue: T): [T, (value: T) => void] {
  const [value, setValue] = useState(resetValue)
  const resetTimerRef = useRef<number | null>(null)
  const mountedRef = useRef(true)

  const clearResetTimer = useCallback(() => {
    if (resetTimerRef.current !== null) {
      window.clearTimeout(resetTimerRef.current)
      resetTimerRef.current = null
    }
  }, [])

  // Why: copy feedback timers are event-owned, but still need unmount cleanup
  // so delayed clipboard/timer work cannot update a destroyed component.
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      clearResetTimer()
    }
  }, [clearResetTimer])

  const showFeedback = useCallback(
    (nextValue: T) => {
      if (!mountedRef.current) {
        return
      }
      clearResetTimer()
      setValue(nextValue)
      resetTimerRef.current = window.setTimeout(() => {
        if (!mountedRef.current) {
          return
        }
        setValue(resetValue)
        resetTimerRef.current = null
      }, 1500)
    },
    [clearResetTimer, resetValue]
  )

  return [value, showFeedback]
}

type GitStatusSourceControlTreeNode = SourceControlTreeNode<
  GitStatusEntry,
  (typeof SECTION_ORDER)[number]
>

export type SourceControlTreeDirectoryNode = Extract<GitStatusSourceControlTreeNode, { type: 'directory' }>

export type SourceControlDirectoryActionPaths = {
  stagePaths: string[]
  unstagePaths: string[]
  discardPaths: string[]
}

type PullRequestComposerProps = {
  provider: HostedReviewProvider
  branch: string
  base: string
  setBase: (value: string) => void
  title: string
  setTitle: (value: string) => void
  body: string
  setBody: (value: string) => void
  draft: boolean
  setDraft: (value: boolean) => void
  baseQuery: string
  setBaseQuery: (value: string) => void
  baseResults: string[]
  setBaseResults: (value: string[]) => void
  baseSearchError: string | null
  aiGenerationEnabled: boolean
  generating: boolean
  generateDisabled: boolean
  generateDisabledReason?: string
  generateError: string | null
  instructionGuidance?: SourceControlAiInstructionGuidance
  createError: string | null
  isCreating: boolean
  primaryAction: PrimaryAction
  dropdownItems: DropdownEntry[]
  onGenerate: () => void
  onCancelGenerate: () => void
  onPrimaryAction: () => void
  onDropdownAction: (kind: DropdownActionKind) => void
}

function SourceControlAiInstructionGuidanceButton({
  guidance
}: {
  guidance: SourceControlAiInstructionGuidance
}): React.JSX.Element {
  const label =
    guidance.operation === 'commitMessage'
      ? 'Add commit message instructions'
      : 'Add pull request instructions'
  const target = guidance.repoBacked
    ? 'Repo Settings > Source Control AI'
    : 'Settings > Git > Source Control AI'
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex size-5 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          aria-label={label}
          title={label}
        >
          <Settings2 className="size-3.5" />
        </button>
      </PopoverTrigger>
      <PopoverContent side="left" sideOffset={6} className="w-64 space-y-2 p-3">
        <div className="space-y-1">
          <p className="text-xs font-medium text-foreground">{label}</p>
          <p className="text-[11px] text-muted-foreground">
            No instructions are configured for this generator. Add them in {target}.
          </p>
        </div>
        <Button
          type="button"
          variant="secondary"
          size="xs"
          className="w-full"
          onClick={guidance.onOpenSettings}
        >
          Open settings
        </Button>
      </PopoverContent>
    </Popover>
  )
}

export function PullRequestComposer({
  provider,
  branch,
  base,
  setBase,
  title,
  setTitle,
  body,
  setBody,
  draft,
  setDraft,
  baseQuery,
  setBaseQuery,
  baseResults,
  setBaseResults,
  baseSearchError,
  aiGenerationEnabled,
  generating,
  generateDisabled,
  generateDisabledReason,
  generateError,
  instructionGuidance,
  createError,
  isCreating,
  primaryAction,
  dropdownItems,
  onGenerate,
  onCancelGenerate,
  onPrimaryAction,
  onDropdownAction
}: PullRequestComposerProps): React.JSX.Element {
  const copy = hostedReviewCreationCopy(provider)
  const ReviewIcon = provider === 'gitlab' ? GitMerge : GitPullRequestArrow
  const normalizedBase = stripBaseRef(base)
  const strippedBranch = stripBaseRef(branch)
  const baseSameAsBranch = normalizedBase.toLowerCase() === strippedBranch.toLowerCase()
  const createDisabled =
    primaryAction.disabled ||
    generating ||
    title.trim().length === 0 ||
    normalizedBase.trim().length === 0 ||
    baseSameAsBranch
  // Why: surface a concrete reason on the disabled Create PR button so the
  // user knows what's blocking submission instead of a silent gray state.
  let createDisabledReason: string | undefined
  if (generating) {
    createDisabledReason = 'Wait for AI generation to finish.'
  } else if (title.trim().length === 0) {
    createDisabledReason = `Enter a ${copy.reviewLabel} title.`
  } else if (normalizedBase.trim().length === 0) {
    createDisabledReason = 'Choose a base branch.'
  } else if (baseSameAsBranch) {
    createDisabledReason = 'Base branch must differ from the head branch.'
  }

  // Why: lock the title/body/base inputs while AI generation is running so
  // the user can't race the request — the hook otherwise rejects the result
  // with "Fields changed while generating" and silently drops the draft.
  const fieldsLocked = generating

  return (
    <div className="px-3 pb-2">
      <div className="space-y-2.5">
        <div className="flex min-w-0 items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-1.5 text-xs">
            <ReviewIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            <span className="font-medium text-foreground">New {copy.reviewLabel}</span>
          </div>
          {aiGenerationEnabled ? (
            generating ? (
              <button
                type="button"
                onClick={() => onCancelGenerate()}
                className="inline-flex h-6 shrink-0 items-center gap-1 rounded-md border border-border bg-background px-2 text-[11px] text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                title="Stop generating"
                aria-label={`Stop generating ${copy.reviewLabel} details`}
              >
                <RefreshCw className="size-3 animate-spin" />
                <span>Generating…</span>
                <Square className="size-2.5 fill-current" />
              </button>
            ) : (
              <button
                type="button"
                disabled={generateDisabled}
                onClick={() => onGenerate()}
                className="inline-flex h-6 shrink-0 items-center gap-1 rounded-md border border-border bg-background px-2 text-[11px] font-medium text-foreground transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-background"
                title={generateDisabledReason ?? `Generate ${copy.reviewLabel} details with AI`}
                aria-label={`Generate ${copy.reviewLabel} details with AI`}
              >
                <Sparkles className="size-3" />
                Generate
              </button>
            )
          ) : null}
          {instructionGuidance ? (
            <SourceControlAiInstructionGuidanceButton guidance={instructionGuidance} />
          ) : null}
        </div>

        {/* Why: a single line that shows the head→base flow plain-language so
            the user can sanity-check the merge direction at a glance. */}
        <div className="flex min-w-0 items-center gap-1.5 text-[11px] text-muted-foreground">
          <span className="truncate font-mono text-foreground" title={strippedBranch}>
            {strippedBranch}
          </span>
          <ArrowDownUp className="size-3 rotate-90 shrink-0 opacity-60" aria-hidden="true" />
          <span
            className={cn(
              'truncate font-mono',
              baseSameAsBranch ? 'text-destructive' : 'text-foreground'
            )}
            title={normalizedBase || 'base'}
          >
            {normalizedBase || 'base'}
          </span>
        </div>

        <div className="relative space-y-2">
          <input
            aria-label={`${copy.titleLabel} title`}
            value={title}
            disabled={fieldsLocked}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="Title"
            className="h-8 w-full min-w-0 rounded-md border border-border bg-background px-2 text-xs font-medium text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
          />

          <textarea
            aria-label={`${copy.titleLabel} description`}
            rows={6}
            value={body}
            disabled={fieldsLocked}
            onChange={(event) => setBody(event.target.value)}
            placeholder="Description (optional)"
            className="min-h-[7.5rem] w-full resize-y rounded-md border border-border bg-background px-2 py-1.5 text-xs text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60 scrollbar-sleek"
          />

          {generating ? (
            // Why: visible scrim + status row so the user understands the
            // title and description fields will be replaced when generation
            // finishes; locking the inputs above also prevents the
            // "Fields changed while generating" race in the hook.
            <div
              className="pointer-events-none absolute inset-0 flex items-center justify-center rounded-md bg-background/40"
              aria-hidden="true"
            >
              <div className="pointer-events-auto flex items-center gap-1.5 rounded-md border border-border bg-background px-2 py-1 text-[11px] text-muted-foreground shadow-sm">
                <Sparkles className="size-3 animate-pulse text-foreground" />
                <span>Generating title & description…</span>
              </div>
            </div>
          ) : null}
        </div>

        {/* Why: base picker as its own labeled row so the title input can use
            the full width. The dropdown chevron makes the picker affordance
            obvious; the inline label clarifies that this is the merge target. */}
        <div className="flex items-center gap-2">
          <span className="shrink-0 text-[11px] text-muted-foreground">Base</span>
          <div className="relative min-w-0 flex-1">
            <input
              aria-label={`${copy.titleLabel} base branch`}
              value={baseQuery || base}
              disabled={fieldsLocked}
              onChange={(event) => {
                setBaseQuery(event.target.value)
                setBase(event.target.value)
              }}
              placeholder="main"
              className="h-7 w-full min-w-0 rounded-md border border-border bg-background px-2 pr-6 font-mono text-xs text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
            />
            <ChevronDown
              className="pointer-events-none absolute right-1.5 top-1.5 size-3.5 text-muted-foreground"
              aria-hidden="true"
            />
          </div>
        </div>

        <label
          className={cn(
            'flex h-7 items-center gap-2 rounded-md border border-border bg-background px-2 text-xs text-foreground transition-colors',
            fieldsLocked
              ? 'cursor-not-allowed opacity-60'
              : 'cursor-pointer hover:bg-accent hover:text-accent-foreground'
          )}
        >
          <input
            type="checkbox"
            checked={draft}
            disabled={fieldsLocked}
            onChange={(event) => setDraft(event.target.checked)}
            className="size-3.5 shrink-0 rounded border-border accent-primary"
          />
          <span className="min-w-0 flex-1 truncate">Create as draft</span>
        </label>

        {baseResults.length > 0 ? (
          <div className="max-h-28 overflow-auto rounded-md border border-border p-1 scrollbar-sleek">
            {baseResults.map((ref) => (
              <button
                key={ref}
                type="button"
                className={cn(
                  'flex w-full items-center justify-between rounded-sm px-2 py-1.5 text-left font-mono text-xs hover:bg-accent',
                  stripBaseRef(base) === ref && 'bg-accent text-accent-foreground'
                )}
                onClick={() => {
                  setBase(ref)
                  setBaseQuery('')
                  setBaseResults([])
                }}
              >
                <span className="truncate">{ref}</span>
                {stripBaseRef(base) === ref ? <Check className="size-3" /> : null}
              </button>
            ))}
          </div>
        ) : null}

        <div className="flex items-stretch pt-0.5">
          <Button
            type="button"
            size="xs"
            disabled={createDisabled}
            onClick={() => onPrimaryAction()}
            className="h-7 flex-1 rounded-r-none px-3 text-xs"
            title={createDisabledReason ?? primaryAction.title}
          >
            {isCreating ? (
              <RefreshCw className="size-3.5 animate-spin" />
            ) : (
              <ReviewIcon className="size-3.5" />
            )}
            {isCreating
              ? 'Creating...'
              : draft
                ? `Create draft ${copy.shortLabel}`
                : `Create ${copy.shortLabel}`}
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                size="xs"
                className={cn(
                  'h-7 rounded-l-none border-l border-primary-foreground/20 px-1.5 shrink-0',
                  createDisabled && 'opacity-50'
                )}
                aria-label={`More ${copy.reviewLabel} and remote actions`}
                title="More actions"
              >
                <ChevronDown className="size-3.5" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-[14rem]">
              {dropdownItems.map((entry, index) =>
                entry.kind === 'separator' ? (
                  <DropdownMenuSeparator key={`sep-${index}`} />
                ) : (
                  <DropdownMenuItem
                    key={entry.kind}
                    disabled={entry.disabled}
                    title={entry.title}
                    variant={entry.variant}
                    onSelect={(event) => {
                      if (entry.disabled) {
                        event.preventDefault()
                        return
                      }
                      onDropdownAction(entry.kind)
                    }}
                  >
                    <span className="flex min-w-0 flex-col">
                      <span>{entry.label}</span>
                      {entry.hint ? (
                        <span className="truncate text-[10px] text-muted-foreground">
                          {entry.hint}
                        </span>
                      ) : null}
                    </span>
                  </DropdownMenuItem>
                )
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {baseSameAsBranch ? (
          <p className="flex items-start gap-1 text-[11px] text-destructive">
            <TriangleAlert className="mt-px size-3 shrink-0" aria-hidden="true" />
            <span>Choose a different base branch before creating a {copy.reviewLabel}.</span>
          </p>
        ) : null}
        {baseSearchError ? (
          <p className="flex items-start gap-1 text-[11px] text-destructive">
            <TriangleAlert className="mt-px size-3 shrink-0" aria-hidden="true" />
            <span>{baseSearchError}</span>
          </p>
        ) : null}
        {generateError ? (
          <p className="flex items-start gap-1 text-[11px] text-destructive">
            <TriangleAlert className="mt-px size-3 shrink-0" aria-hidden="true" />
            <span>{generateError}</span>
          </p>
        ) : null}
        {createError ? (
          <p className="flex items-start gap-1 text-[11px] text-destructive">
            <TriangleAlert className="mt-px size-3 shrink-0" aria-hidden="true" />
            <span>{createError}</span>
          </p>
        ) : null}
      </div>
    </div>
  )
}

type CommitFailureFixSplitButtonProps = {
  label: string
  worktreeId: string | null
  groupId: string | null
  prompt: string | null
  isLaunching: boolean
  variant: React.ComponentProps<typeof Button>['variant']
  size: React.ComponentProps<typeof Button>['size']
  iconClassName: string
  primaryClassName?: string
  chevronClassName?: string
  onFixWithDefaultAgent: (promptOverride?: string) => Promise<boolean> | boolean
  onPromptDelivered: () => void
}

function CommitFailureFixSplitButton({
  label,
  worktreeId,
  groupId,
  prompt,
  isLaunching,
  variant,
  size,
  iconClassName,
  primaryClassName,
  chevronClassName,
  onFixWithDefaultAgent,
  onPromptDelivered
}: CommitFailureFixSplitButtonProps): React.JSX.Element {
  const [customizePromptOpen, setCustomizePromptOpen] = useState(false)
  const [customInstruction, setCustomInstruction] = useState('')
  const customInstructionId = React.useId()
  const canLaunch = Boolean(worktreeId && groupId && prompt)
  const hasCustomInstruction = customInstruction.trim().length > 0
  const customizedPrompt = useMemo(
    () => (prompt ? appendCommitFailureCustomInstruction(prompt, customInstruction) : null),
    [customInstruction, prompt]
  )
  const dividerClass = variant === 'default' ? 'border-primary-foreground/20' : 'border-border'
  const handleCustomizePromptOpenChange = useCallback((open: boolean) => {
    setCustomizePromptOpen(open)
    if (!open) {
      setCustomInstruction('')
    }
  }, [])
  const handleStartDefaultWithCustomPrompt = useCallback(async () => {
    if (!customizedPrompt || !hasCustomInstruction) {
      return
    }
    const launched = await onFixWithDefaultAgent(customizedPrompt)
    if (launched) {
      setCustomizePromptOpen(false)
      setCustomInstruction('')
    }
  }, [customizedPrompt, hasCustomInstruction, onFixWithDefaultAgent])
  const handleCustomPromptDelivered = useCallback(() => {
    setCustomizePromptOpen(false)
    setCustomInstruction('')
    onPromptDelivered()
  }, [onPromptDelivered])

  return (
    <>
      <DropdownMenu>
        <div className="flex shrink-0 items-stretch">
          <Button
            type="button"
            variant={variant}
            size={size}
            className={cn('rounded-r-none', primaryClassName)}
            disabled={isLaunching || !canLaunch}
            onClick={() => void onFixWithDefaultAgent()}
            title="Start the default AI agent to fix this commit failure"
            aria-label="Fix commit failure with AI"
          >
            {isLaunching ? (
              <RefreshCw className={cn(iconClassName, 'animate-spin')} />
            ) : (
              <Sparkle className={iconClassName} />
            )}
            {label}
          </Button>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant={variant}
              size={size}
              className={cn('rounded-l-none border-l', dividerClass, chevronClassName)}
              disabled={isLaunching || !canLaunch}
              title="Choose an agent for this commit failure"
              aria-label="Choose agent to fix commit failure"
            >
              <ChevronDown className={iconClassName} />
            </Button>
          </DropdownMenuTrigger>
        </div>
        <DropdownMenuContent align="end" className="min-w-[210px] p-1">
          {worktreeId && groupId && prompt ? (
            <>
              <DropdownMenuItem
                onSelect={() => setCustomizePromptOpen(true)}
                className="gap-2 rounded-[7px] px-2 py-1.5 text-[12px] leading-5 font-medium"
              >
                <PencilLine className="size-4 text-muted-foreground" />
                Customize prompt...
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <QuickLaunchAgentMenuItems
                worktreeId={worktreeId}
                groupId={groupId}
                onFocusTerminal={focusTerminalTabSurface}
                prompt={prompt}
                promptDelivery="submit-after-ready"
                launchSource="source_control_recovery"
                onPromptDelivered={onPromptDelivered}
              />
            </>
          ) : (
            <DropdownMenuItem disabled>Commit failure context unavailable</DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={customizePromptOpen} onOpenChange={handleCustomizePromptOpenChange}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Customize Prompt</DialogTitle>
            <DialogDescription>Add one-time guidance for this failed commit.</DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Label htmlFor={customInstructionId} className="text-xs">
              Custom instruction
            </Label>
            <textarea
              id={customInstructionId}
              value={customInstruction}
              onChange={(event) => setCustomInstruction(event.target.value)}
              placeholder="Focus on the staged files only, and prefer the smallest lint-safe change."
              rows={5}
              className="w-full resize-none rounded-md border border-border bg-background px-2.5 py-2 text-xs text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:ring-1 focus-visible:ring-ring"
            />
          </div>
          <DialogFooter className="gap-2">
            <DialogClose asChild>
              <Button type="button" variant="outline" size="sm">
                Cancel
              </Button>
            </DialogClose>
            {worktreeId && groupId && customizedPrompt ? (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={isLaunching || !hasCustomInstruction}
                  >
                    Choose agent
                    <ChevronDown className="size-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="min-w-[180px] p-1">
                  <QuickLaunchAgentMenuItems
                    worktreeId={worktreeId}
                    groupId={groupId}
                    onFocusTerminal={focusTerminalTabSurface}
                    prompt={customizedPrompt}
                    promptDelivery="submit-after-ready"
                    launchSource="source_control_recovery"
                    onPromptDelivered={handleCustomPromptDelivered}
                  />
                </DropdownMenuContent>
              </DropdownMenu>
            ) : null}
            <Button
              type="button"
              size="sm"
              disabled={isLaunching || !canLaunch || !hasCustomInstruction}
              onClick={() => void handleStartDefaultWithCustomPrompt()}
            >
              {isLaunching ? (
                <RefreshCw className="size-4 animate-spin" />
              ) : (
                <Sparkle className="size-4" />
              )}
              Start default agent
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

type CommitAreaProps = {
  worktreeId: string | null
  groupId: string | null
  commitMessage: string
  commitError: string | null
  commitFailureRecoveryPrompt: string | null
  remoteActionError: string | null
  isCommitting: boolean
  isFixingCommitFailureWithAI: boolean
  isCreatingPr?: boolean
  showComposer?: boolean
  aiEnabled: boolean
  aiAgentConfigured: boolean
  isGenerating: boolean
  generateError: string | null
  instructionGuidance?: SourceControlAiInstructionGuidance
  stagedCount: number
  hasUnresolvedConflicts: boolean
  isRemoteOperationActive: boolean
  inFlightRemoteOpKind: RemoteOpKind | null
  primaryAction: PrimaryAction
  dropdownItems: DropdownEntry[]
  onCommitMessageChange: (message: string) => void
  onGenerate: () => void
  onCancelGenerate: () => void
  onFixCommitFailureWithAI: (promptOverride?: string) => Promise<boolean> | boolean
  onPrimaryAction: () => void
  onDropdownAction: (kind: DropdownActionKind) => void
}

export function CommitArea({
  worktreeId,
  groupId,
  commitMessage,
  commitError,
  commitFailureRecoveryPrompt,
  remoteActionError,
  isCommitting,
  isFixingCommitFailureWithAI,
  isCreatingPr = false,
  showComposer = true,
  aiEnabled,
  aiAgentConfigured,
  isGenerating,
  generateError,
  instructionGuidance,
  stagedCount,
  hasUnresolvedConflicts,
  isRemoteOperationActive,
  inFlightRemoteOpKind,
  primaryAction,
  dropdownItems,
  onCommitMessageChange,
  onGenerate,
  onCancelGenerate,
  onFixCommitFailureWithAI,
  onPrimaryAction,
  onDropdownAction
}: CommitAreaProps): React.JSX.Element {
  // Why: cap at 12 rows so a pasted multi-page commit message doesn't push
  // the Commit button off-screen. The textarea keeps `resize-none` (matching
  // the existing style) — the browser scrolls internally past 12 rows.
  const rows = Math.min(12, Math.max(2, commitMessage.split('\n').length))
  // Why: only spin the primary when its label matches what's actually
  // running. resolvePrimaryAction overrides the primary kind to mirror the
  // in-flight op (e.g. user picks Sync from the dropdown → primary becomes
  // "Sync"), so the equality check spins the button for any primary-
  // eligible remote op the user triggered. Background ops the primary
  // doesn't show (Fetch) leave primaryAction.kind unchanged and the
  // mismatch keeps the spinner off — the disabled state alone is enough
  // signal there. Commit still spins on isCommitting because that path
  // doesn't go through inFlightRemoteOpKind.
  const showSpinner =
    primaryAction.kind === 'create_pr'
      ? isCreatingPr
      : primaryAction.kind === 'commit'
        ? isCommitting
        : isRemoteOperationActive && primaryAction.kind === inFlightRemoteOpKind
  // Why: when the primary doesn't host the in-flight op (e.g. Fetch, or any
  // dropdown action that mismatches the primary's natural label) the click
  // would otherwise be silent — the toast only fires on failure and a
  // no-op fetch leaves status counts unchanged. Spinning the chevron gives
  // the user immediate feedback that the action they picked is running,
  // while still leaving the menu reachable to read the disabled-row
  // tooltips.
  const showChevronSpinner =
    (isCommitting || isCreatingPr || isRemoteOperationActive) && !showSpinner
  const commitFailureSummary = useMemo(
    () => (commitError ? summarizeCommitFailure(commitError) : null),
    [commitError]
  )
  const commitFailureKindLabel = useMemo(
    () => (commitFailureSummary ? getCommitFailureKindLabel(commitFailureSummary) : null),
    [commitFailureSummary]
  )
  const hasCommitFailureDetails = useMemo(
    () =>
      commitError && commitFailureSummary
        ? hasExpandedCommitFailureDetails(commitError, commitFailureSummary)
        : false,
    [commitError, commitFailureSummary]
  )
  // Why: the details dialog is scoped to the worktree, not the exact stderr
  // text, so a retried commit can refresh an open dialog with newer output.
  const commitFailureWorktreeKey = getCommitFailureDialogWorktreeKey(worktreeId)
  const [commitFailureDialogState, setCommitFailureDialogState] =
    useState<CommitFailureDialogState>({
      worktreeKey: commitFailureWorktreeKey,
      open: false
    })
  const isCommitFailureDialogOpen = shouldShowCommitFailureDialog(
    commitFailureDialogState,
    commitFailureWorktreeKey,
    hasCommitFailureDetails
  )
  const setCommitFailureDialogOpen = useCallback(
    (open: boolean) => {
      setCommitFailureDialogState({ worktreeKey: commitFailureWorktreeKey, open })
    },
    [commitFailureWorktreeKey]
  )
  const handleFixCommitFailureWithAI = useCallback(
    async (promptOverride?: string): Promise<boolean> => {
      const launched = await onFixCommitFailureWithAI(promptOverride)
      if (launched) {
        setCommitFailureDialogOpen(false)
      }
      return launched
    },
    [onFixCommitFailureWithAI, setCommitFailureDialogOpen]
  )
  const handleCommitFailureAgentPromptDelivered = useCallback(() => {
    setCommitFailureDialogOpen(false)
  }, [setCommitFailureDialogOpen])

  useEffect(() => {
    setCommitFailureDialogState((current) =>
      syncCommitFailureDialogState(current, commitFailureWorktreeKey, hasCommitFailureDetails)
    )
  }, [commitFailureWorktreeKey, hasCommitFailureDetails])

  // Why: most primary-kind labels are anchored by a directional icon so
  // the affirmative Commit (✓) reads distinctly from the remote-state
  // labels sharing this slot — Push (↑), Sync (↕), Publish (☁︎↑). Pull is
  // intentionally icon-less because the down-arrow read as a
  // download/save affordance. The icon is decorative; the label and
  // title attribute carry the meaning for assistive tech.
  const PrimaryIcon = PRIMARY_ICONS[primaryAction.kind]

  const hasMessage = commitMessage.trim().length > 0
  const describedBy = [
    commitError ? 'commit-area-error' : null,
    remoteActionError ? 'commit-area-remote-error' : null,
    generateError ? 'commit-area-generate-error' : null
  ]
    .filter(Boolean)
    .join(' ')

  // Why: only render the Generate button when the user has opted into the
  // feature. Mounting a perma-disabled button would leak space and add noise
  // for users who never plan to use AI commit messages.
  const showGenerate = showComposer && aiEnabled
  let generateDisabledReason: string | undefined
  if (isGenerating) {
    generateDisabledReason = 'Generating commit message…'
  } else if (isCommitting) {
    generateDisabledReason = 'Commit in progress…'
  } else if (!aiAgentConfigured) {
    generateDisabledReason = 'Pick an agent in Settings -> Git -> Source Control AI.'
  } else if (stagedCount === 0) {
    generateDisabledReason = 'Stage at least one file to generate a message.'
  } else if (hasMessage) {
    generateDisabledReason = 'Clear the message to regenerate.'
  }
  const isGenerateDisabled =
    !aiAgentConfigured ||
    isGenerating ||
    isCommitting ||
    stagedCount === 0 ||
    hasMessage ||
    hasUnresolvedConflicts

  return (
    <div className="px-3 pb-2">
      {showComposer ? (
        <div className="relative">
          <textarea
            rows={rows}
            value={commitMessage}
            onChange={(e) => onCommitMessageChange(e.target.value)}
            placeholder="Message"
            aria-label="Commit message"
            aria-describedby={describedBy || undefined}
            // Why: reserve right padding so typed text does not slide under the
            // absolute-positioned Generate icon in the top-right corner.
            className={`mt-0.5 w-full resize-none rounded-md border border-border bg-background px-2 py-1.5 text-xs text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:ring-1 focus-visible:ring-ring ${
              showGenerate ? (instructionGuidance ? 'pr-12' : 'pr-7') : ''
            }`}
          />
          {showGenerate &&
            (isGenerating ? (
              // Why: while generating the icon doubles as the cancel affordance.
              // Default state shows the spinning RefreshCw; on hover/focus we
              // swap to a Square ("stop") with a destructive tint so the user
              // sees that clicking will abort the run. Group/group-hover toggles
              // keep this stateless on the React side.
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    onClick={() => onCancelGenerate()}
                    title="Stop generating"
                    aria-label="Stop generating commit message"
                    className="group absolute right-1.5 top-1.5 inline-flex size-5 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:bg-destructive/10 focus-visible:text-destructive focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-destructive/40"
                  >
                    <RefreshCw className="size-3.5 animate-spin group-hover:hidden group-focus-visible:hidden" />
                    <Square className="hidden size-3.5 fill-current group-hover:block group-focus-visible:block" />
                  </button>
                </TooltipTrigger>
                <TooltipContent side="left" sideOffset={6}>
                  Generating commit message. Click to stop.
                </TooltipContent>
              </Tooltip>
            ) : (
              <div className="absolute right-1.5 top-1.5 flex items-center gap-0.5">
                {instructionGuidance ? (
                  <SourceControlAiInstructionGuidanceButton guidance={instructionGuidance} />
                ) : null}
                <button
                  type="button"
                  disabled={isGenerateDisabled}
                  onClick={() => onGenerate()}
                  title={generateDisabledReason ?? 'Generate commit message with AI'}
                  aria-label="Generate commit message with AI"
                  className="inline-flex size-5 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground"
                >
                  <Sparkles className="size-3.5" />
                </button>
              </div>
            ))}
        </div>
      ) : null}
      {/* Why: primary + chevron sit together as a visual split button so the
          edit → commit → push loop stays in a single vertical band. The
          chevron exposes the full action surface (fetch, pull, sync,
          publish, compound commits) without forcing morphing labels to
          carry every possible intent. */}
      <div className={cn(showComposer ? 'mt-1 flex items-stretch' : 'flex items-stretch')}>
        {/* Why: match the hosted-review action buttons in Checks
            (size="xs", px-3 text-[11px]) so the sidebar has a consistent
            action-button shape across Source Control and Checks. The primary
            and chevron share a single rounded rectangle — rounded-r-none on
            the primary and rounded-l-none + border-l on the chevron make the
            pair read as one split button instead of two detached buttons. */}
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="flex flex-1">
              <Button
                type="button"
                size="xs"
                disabled={primaryAction.disabled}
                onClick={() => onPrimaryAction()}
                className="w-full rounded-r-none px-3 text-[11px]"
                title={primaryAction.title}
              >
                {showSpinner ? (
                  <RefreshCw className="size-3.5 animate-spin" />
                ) : PrimaryIcon ? (
                  <PrimaryIcon className="size-3.5" aria-hidden="true" />
                ) : null}
                {primaryAction.label}
              </Button>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top" sideOffset={6} className="max-w-72">
            {primaryAction.title}
          </TooltipContent>
        </Tooltip>
        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="inline-flex shrink-0">
                <DropdownMenuTrigger asChild>
                  <Button
                    type="button"
                    size="xs"
                    className={cn(
                      'rounded-l-none border-l border-primary-foreground/20 px-1.5 shrink-0',
                      // Why: mirror the primary's disabled dimming so the split
                      // button reads as one unit when Commit is unavailable. The
                      // chevron itself stays clickable — its dropdown exposes
                      // independently-gated remote actions (push / fetch / pull)
                      // that are still valid when the primary is disabled.
                      primaryAction.disabled && 'opacity-50'
                    )}
                    aria-label="More commit and remote actions"
                    title="More actions"
                  >
                    {showChevronSpinner ? (
                      <RefreshCw className="size-3.5 animate-spin" />
                    ) : (
                      <ChevronDown className="size-3.5" />
                    )}
                  </Button>
                </DropdownMenuTrigger>
              </span>
            </TooltipTrigger>
            <TooltipContent side="top" sideOffset={6}>
              More commit and remote actions
            </TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" className="min-w-[14rem]">
            {dropdownItems.map((entry, index) =>
              entry.kind === 'separator' ? (
                <DropdownMenuSeparator key={`sep-${index}`} />
              ) : (
                <Tooltip key={entry.kind}>
                  <TooltipTrigger asChild>
                    <div className="block">
                      <DropdownMenuItem
                        disabled={entry.disabled}
                        title={entry.title}
                        variant={entry.variant}
                        className="w-full"
                        onSelect={(event) => {
                          if (entry.disabled) {
                            event.preventDefault()
                            return
                          }
                          onDropdownAction(entry.kind)
                        }}
                      >
                        <span className="flex min-w-0 flex-col">
                          <span>{entry.label}</span>
                          {entry.hint ? (
                            <span className="truncate text-[10px] text-muted-foreground">
                              {entry.hint}
                            </span>
                          ) : null}
                        </span>
                      </DropdownMenuItem>
                    </div>
                  </TooltipTrigger>
                  <TooltipContent side="left" sideOffset={8} className="max-w-72">
                    {entry.title}
                  </TooltipContent>
                </Tooltip>
              )
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {commitError && (
        // Why: role="alert" + aria-live="polite" lets screen readers announce
        // commit failures; the id ties the message to the textarea via
        // aria-describedby so assistive tech associates the two.
        <div
          id="commit-area-error"
          role="alert"
          aria-live="polite"
          className="mt-2 min-w-0 overflow-hidden rounded-lg border border-destructive/20 bg-card text-card-foreground shadow-xs"
        >
          <div className="h-0.5 bg-destructive/70" aria-hidden="true" />
          <div className="grid min-w-0 gap-2 px-2.5 py-2.5">
            <div className="grid min-w-0 grid-cols-[1rem_minmax(0,1fr)] gap-1.5">
              <span className="mt-px inline-flex size-4 shrink-0 items-center justify-center rounded-full bg-destructive/10 text-destructive">
                <TriangleAlert className="size-3" aria-hidden="true" />
              </span>
              <div className="flex min-w-0 items-center gap-1.5">
                <span className="text-xs font-semibold text-foreground">Commit blocked</span>
                {commitFailureKindLabel ? (
                  <span className="shrink-0 rounded-full bg-destructive/10 px-1.5 py-px text-[10px] leading-4 font-semibold text-destructive">
                    {commitFailureKindLabel}
                  </span>
                ) : null}
              </div>
              <p className="col-start-2 mt-0.5 line-clamp-3 min-w-0 font-mono text-[11px] leading-4 break-words text-muted-foreground [overflow-wrap:anywhere]">
                {commitFailureSummary}
              </p>
            </div>
            <div className="ml-[1.375rem] flex min-w-0 items-center gap-1.5">
              <CommitFailureFixSplitButton
                label="AI Fix"
                worktreeId={worktreeId}
                groupId={groupId}
                prompt={commitFailureRecoveryPrompt}
                isLaunching={isFixingCommitFailureWithAI}
                variant="secondary"
                size="xs"
                iconClassName="size-3"
                primaryClassName="h-6 px-2 text-[11px]"
                chevronClassName="h-6 px-1.5"
                onFixWithDefaultAgent={handleFixCommitFailureWithAI}
                onPromptDelivered={handleCommitFailureAgentPromptDelivered}
              />
              {hasCommitFailureDetails && (
                <Button
                  type="button"
                  variant="outline"
                  size="xs"
                  className="h-6 shrink-0 border-foreground/25 px-2 text-[11px] font-semibold"
                  onClick={() => setCommitFailureDialogOpen(true)}
                >
                  Details
                </Button>
              )}
            </div>
          </div>
        </div>
      )}
      {commitError && commitFailureSummary && hasCommitFailureDetails && (
        <Dialog
          key={commitFailureWorktreeKey}
          open={isCommitFailureDialogOpen}
          onOpenChange={setCommitFailureDialogOpen}
        >
          <DialogContent className="sm:max-w-2xl">
            <DialogHeader>
              <DialogTitle>Commit Failed</DialogTitle>
              <DialogDescription>{commitFailureSummary}</DialogDescription>
            </DialogHeader>
            <pre className="max-h-[60vh] overflow-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-xs whitespace-pre-wrap text-foreground scrollbar-sleek">
              {commitError}
            </pre>
            <DialogFooter>
              <CommitFailureFixSplitButton
                label="Fix with AI"
                worktreeId={worktreeId}
                groupId={groupId}
                prompt={commitFailureRecoveryPrompt}
                isLaunching={isFixingCommitFailureWithAI}
                variant="default"
                size="sm"
                iconClassName="size-4"
                primaryClassName="rounded-r-none"
                chevronClassName="rounded-l-none border-l border-primary-foreground/20 px-2"
                onFixWithDefaultAgent={handleFixCommitFailureWithAI}
                onPromptDelivered={handleCommitFailureAgentPromptDelivered}
              />
              <DialogClose asChild>
                <Button type="button" variant="outline" size="sm">
                  Close
                </Button>
              </DialogClose>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
      {remoteActionError && (
        <p
          id="commit-area-remote-error"
          role="alert"
          aria-live="polite"
          className="mt-1 text-[11px] text-destructive"
        >
          {remoteActionError}
        </p>
      )}
      {generateError && (
        <p
          id="commit-area-generate-error"
          role="alert"
          aria-live="polite"
          className="mt-1 text-[11px] text-destructive"
        >
          {generateError}
        </p>
      )}
    </div>
  )
}

export function CompareSummary({
  summary,
  viewMode,
  onChangeBaseRef,
  onToggleViewMode,
  viewModeToggleDisabled,
  onRetry
}: {
  summary: GitBranchCompareSummary | null
  viewMode: SourceControlViewMode
  onChangeBaseRef: () => void
  onToggleViewMode: () => void
  viewModeToggleDisabled?: boolean
  onRetry: () => void
}): React.JSX.Element {
  if (!summary || summary.status === 'loading') {
    return (
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <RefreshCw className="size-3.5 animate-spin" />
        <span>Comparing against {summary?.baseRef ?? '…'}</span>
      </div>
    )
  }

  if (summary.status !== 'ready') {
    return (
      <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
        <span className="min-w-0 flex-1 truncate">
          {summary.errorMessage ?? 'Branch compare unavailable'}
        </span>
        <div className="flex shrink-0 items-center gap-2">
          <CompareSummaryToolbarButton
            icon={Settings2}
            label="Change base ref"
            onClick={onChangeBaseRef}
          />
          <CompareSummaryToolbarButton
            icon={viewMode === 'tree' ? List : ListTree}
            label={viewMode === 'tree' ? 'Show changes as list' : 'Show changes as tree'}
            onClick={onToggleViewMode}
            disabled={viewModeToggleDisabled}
          />
          <CompareSummaryToolbarButton icon={RefreshCw} label="Retry" onClick={onRetry} />
        </div>
      </div>
    )
  }

  return (
    <div className="flex items-center gap-2 text-xs text-muted-foreground">
      {summary.commitsAhead !== undefined && (
        <span title={`Comparing against ${summary.baseRef}`}>
          {summary.commitsAhead} commits ahead of {summary.baseRef}
        </span>
      )}
      <div className="ml-auto flex shrink-0 items-center gap-2">
        <CompareSummaryToolbarButton
          icon={Settings2}
          label="Change base ref"
          onClick={onChangeBaseRef}
        />
        <CompareSummaryToolbarButton
          icon={viewMode === 'tree' ? List : ListTree}
          label={viewMode === 'tree' ? 'Show changes as list' : 'Show changes as tree'}
          onClick={onToggleViewMode}
          disabled={viewModeToggleDisabled}
        />
        <CompareSummaryToolbarButton
          icon={RefreshCw}
          label="Refresh branch compare"
          onClick={onRetry}
        />
      </div>
    </div>
  )
}

export function CompareSummaryToolbarButton({
  icon: Icon,
  label,
  onClick,
  disabled = false
}: {
  icon: LucideIcon
  label: string
  onClick: () => void
  disabled?: boolean
}): React.JSX.Element {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          className={cn(
            'text-muted-foreground hover:text-foreground',
            disabled && 'cursor-not-allowed opacity-50'
          )}
          aria-label={label}
          aria-disabled={disabled}
          onClick={() => {
            if (!disabled) {
              onClick()
            }
          }}
        >
          <Icon className="size-3.5" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={6}>
        {label}
      </TooltipContent>
    </Tooltip>
  )
}

export function DiffCommentsInlineList({
  comments,
  onDelete,
  onClearFile,
  onOpen
}: {
  comments: DiffComment[]
  onDelete: (commentId: string) => void
  onClearFile: (filePath: string) => void
  // Why: clicking the note row navigates the user to that file's diff (or
  // editor as a fallback) and, when a `commentId` is supplied, scrolls the
  // diff to that specific note via the scrollToDiffCommentId UI slice.
  onOpen: (comment: DiffComment) => void
}): React.JSX.Element {
  // Why: group by filePath so the inline list mirrors the structure in the
  // Notes tab — a compact section per file with line-number prefixes.
  const groups = useMemo(() => {
    const map = new Map<string, DiffComment[]>()
    for (const c of comments) {
      const list = map.get(c.filePath) ?? []
      list.push(c)
      map.set(c.filePath, list)
    }
    for (const list of map.values()) {
      list.sort((a, b) => a.lineNumber - b.lineNumber)
    }
    return Array.from(map.entries())
  }, [comments])

  const [copiedId, showCopiedId] = useCopyFeedbackState<string | null>(null)

  const handleCopyOne = useCallback(
    async (c: DiffComment): Promise<void> => {
      try {
        await api.ui.writeClipboardText(formatDiffComment(c))
        showCopiedId(c.id)
      } catch {
        // Why: swallow — clipboard write can fail when the window isn't focused.
      }
    },
    [showCopiedId]
  )

  if (comments.length === 0) {
    return (
      <div className="px-6 py-2 text-[11px] text-muted-foreground">
        Hover over a line in the diff view and click the + to add a note.
      </div>
    )
  }

  return (
    <div className="bg-muted/20">
      {groups.map(([filePath, list]) => (
        <div key={filePath} className="px-3 py-1.5">
          <div className="group/file flex items-center gap-1">
            <button
              type="button"
              className="block min-w-0 flex-1 truncate text-left text-[10px] font-medium text-muted-foreground hover:text-foreground"
              onClick={() => {
                const first = list[0]
                if (first) {
                  onOpen(first)
                }
              }}
              title={`Open ${filePath}`}
            >
              {filePath}
            </button>
            <button
              type="button"
              className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-destructive focus-visible:opacity-100 group-hover/file:opacity-100"
              onClick={() => onClearFile(filePath)}
              title={`Clear notes for ${filePath}`}
              aria-label={`Clear notes for ${filePath}`}
            >
              <Trash2 className="size-3" />
            </button>
          </div>
          <ul className="mt-1 space-y-1">
            {list.map((c) => (
              <li
                key={c.id}
                className="group flex items-center gap-1.5 rounded px-1 py-0.5 hover:bg-accent/40"
              >
                <button
                  type="button"
                  // Why: a single inner button is the click/keyboard target so
                  // the row's action buttons (copy/delete) can stay as
                  // siblings without nesting interactive elements — that
                  // pattern violates ARIA's no-interactive-descendants rule
                  // for buttons and lets bubbled key events from the children
                  // fire the row's open handler.
                  className="flex min-w-0 flex-1 cursor-pointer items-center gap-1.5 rounded text-left"
                  onClick={() => onOpen(c)}
                  title={`Open ${c.filePath} (${getDiffCommentLineLabel(c).toLowerCase()})`}
                  aria-label={`Open note on ${getDiffCommentLineLabel(c).toLowerCase()}`}
                >
                  <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] leading-none tabular-nums text-muted-foreground">
                    {getDiffCommentLineLabel(c, true)}
                  </span>
                  <span className="shrink-0 rounded bg-muted/70 px-1 py-0.5 text-[10px] leading-none text-muted-foreground">
                    {getDiffCommentSource(c) === 'markdown' ? 'MD' : 'Diff'}
                  </span>
                  {c.sentAt ? (
                    <span className="shrink-0 rounded bg-muted/70 px-1 py-0.5 text-[10px] leading-none text-muted-foreground">
                      Sent
                    </span>
                  ) : null}
                  <span className="block min-w-0 flex-1 whitespace-pre-wrap break-words text-[11px] leading-snug text-foreground">
                    {c.body}
                  </span>
                </button>
                <button
                  type="button"
                  className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100"
                  onClick={() => void handleCopyOne(c)}
                  title="Copy note"
                  aria-label={`Copy note on line ${c.lineNumber}`}
                >
                  {copiedId === c.id ? <Check className="size-3" /> : <Copy className="size-3" />}
                </button>
                <button
                  type="button"
                  className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100"
                  onClick={() => onDelete(c.id)}
                  title="Delete note"
                  aria-label={`Delete note on line ${c.lineNumber}`}
                >
                  <Trash className="size-3" />
                </button>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  )
}

export function ConflictSummaryCard({
  conflictOperation,
  unresolvedCount,
  isResolvingWithAI,
  isAbortingOperation = false,
  onAbortOperation,
  onResolveWithAI,
  onReview
}: {
  conflictOperation: GitConflictOperation
  unresolvedCount: number
  isResolvingWithAI: boolean
  isAbortingOperation?: boolean
  onAbortOperation?: (operation: GitConflictOperation) => void
  onResolveWithAI: () => void
  onReview: () => void
}): React.JSX.Element {
  const operationLabel =
    conflictOperation === 'merge'
      ? 'Merge conflicts'
      : conflictOperation === 'rebase'
        ? 'Rebase conflicts'
        : conflictOperation === 'cherry-pick'
          ? 'Cherry-pick conflicts'
          : 'Conflicts'

  return (
    <div className="rounded-md border border-amber-500/25 bg-amber-500/5 px-3 py-2">
      <div className="flex items-start gap-2">
        <TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-600 dark:text-amber-400" />
        <div className="min-w-0 flex-1">
          <div
            className="text-xs font-medium text-foreground"
            aria-live="polite"
          >{`${operationLabel}: ${unresolvedCount} unresolved`}</div>
          <div className="mt-1 text-[11px] text-muted-foreground">
            Resolved files move back to normal changes after they leave the live conflict state.
          </div>
        </div>
      </div>
      <div className="mt-2">
        <Button
          type="button"
          variant="default"
          size="sm"
          className="h-7 w-full text-xs"
          disabled={isResolvingWithAI}
          onClick={onResolveWithAI}
        >
          {isResolvingWithAI ? (
            <RefreshCw className="size-3.5 animate-spin" />
          ) : (
            <Sparkles className="size-3.5" />
          )}
          Resolve with AI
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="mt-1.5 h-7 w-full text-xs"
          onClick={onReview}
        >
          <GitMerge className="size-3.5" />
          Review conflicts
        </Button>
        {(conflictOperation === 'merge' || conflictOperation === 'rebase') && onAbortOperation ? (
          <Button
            type="button"
            variant="destructive"
            size="sm"
            className="mt-1.5 h-7 w-full text-xs"
            disabled={isResolvingWithAI || isAbortingOperation}
            onClick={() => onAbortOperation(conflictOperation)}
          >
            {isAbortingOperation ? <RefreshCw className="size-3.5 animate-spin" /> : null}
            {conflictOperation === 'rebase' ? 'Abort rebase' : 'Abort merge'}
          </Button>
        ) : null}
      </div>
    </div>
  )
}

export function SourceControlTreeDirectoryRow({
  node,
  actionPaths,
  hideBulkActions,
  isExecutingBulk,
  isCollapsed,
  onToggle,
  onRequestDiscardPaths,
  onStagePaths,
  onUnstagePaths
}: {
  node: SourceControlTreeDirectoryNode
  actionPaths: SourceControlDirectoryActionPaths
  hideBulkActions: boolean
  isExecutingBulk: boolean
  isCollapsed: boolean
  onToggle: () => void
  onRequestDiscardPaths: (area: DiscardAllArea, paths: readonly string[]) => void
  onStagePaths: (paths: readonly string[]) => Promise<void>
  onUnstagePaths: (paths: readonly string[]) => Promise<void>
}): React.JSX.Element {
  // Why: filtered tree nodes only contain visible descendants. Folder-wide
  // bulk labels would overpromise if they acted on that filtered subset.
  const canStage = !hideBulkActions && actionPaths.stagePaths.length > 0
  const canUnstage = !hideBulkActions && actionPaths.unstagePaths.length > 0
  const canDiscard = !hideBulkActions && actionPaths.discardPaths.length > 0

  return (
    <div
      className="group relative flex w-full items-center gap-1 pr-3 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
      style={{
        paddingLeft: `${node.depth * SOURCE_CONTROL_TREE_INDENT_PX + SOURCE_CONTROL_TREE_DIRECTORY_PADDING_PX}px`
      }}
    >
      <button
        type="button"
        className="flex min-w-0 flex-1 items-center gap-1 text-left"
        onClick={onToggle}
        aria-expanded={!isCollapsed}
      >
        <ChevronDown
          className={cn('size-3 shrink-0 transition-transform', isCollapsed && '-rotate-90')}
        />
        {isCollapsed ? (
          <Folder className="size-3 shrink-0" />
        ) : (
          <FolderOpen className="size-3 shrink-0" />
        )}
        <span className="min-w-0 flex-1 truncate">{node.name}</span>
      </button>
      <span className="w-4 shrink-0 text-center text-[10px] font-bold tabular-nums text-muted-foreground/80">
        {node.fileCount}
      </span>
      {(canDiscard || canStage || canUnstage) && (
        <div className={SOURCE_CONTROL_ROW_ACTION_OVERLAY_CLASS}>
          {canDiscard && (
            <ActionButton
              icon={node.area === 'untracked' ? Trash : Undo2}
              title={node.area === 'untracked' ? 'Delete untracked in folder' : 'Discard folder'}
              onClick={(event) => {
                event.stopPropagation()
                onRequestDiscardPaths(node.area, actionPaths.discardPaths)
              }}
              disabled={isExecutingBulk}
            />
          )}
          {canStage && (
            <ActionButton
              icon={Plus}
              title="Stage folder"
              onClick={(event) => {
                event.stopPropagation()
                void onStagePaths(actionPaths.stagePaths)
              }}
              disabled={isExecutingBulk}
            />
          )}
          {canUnstage && (
            <ActionButton
              icon={Minus}
              title="Unstage folder"
              onClick={(event) => {
                event.stopPropagation()
                void onUnstagePaths(actionPaths.unstagePaths)
              }}
              disabled={isExecutingBulk}
            />
          )}
        </div>
      )}
    </div>
  )
}

// Why: a compact +added/-removed magnitude lets users gauge change size at a
// glance. Use git decoration tokens so the source-control sidebar follows the
// documented light/dark status palette.
export function DiffLineCounts({
  added,
  removed
}: {
  added?: number
  removed?: number
}): React.JSX.Element | null {
  const hasAdded = typeof added === 'number' && added > 0
  const hasRemoved = typeof removed === 'number' && removed > 0
  if (!hasAdded && !hasRemoved) {
    return null
  }
  return (
    <span className="shrink-0 tabular-nums text-[10px]">
      {hasAdded && <span style={{ color: 'var(--git-decoration-added)' }}>+{added}</span>}
      {hasAdded && hasRemoved && <span> </span>}
      {hasRemoved && <span style={{ color: 'var(--git-decoration-deleted)' }}>-{removed}</span>}
    </span>
  )
}

export function BranchEntryRow({
  entry,
  currentWorktreeId,
  worktreePath,
  depth = 0,
  onRevealInExplorer,
  onOpen,
  commentCount,
  showPathHint = true
}: {
  entry: GitBranchChangeEntry
  currentWorktreeId: string
  worktreePath: string
  depth?: number
  onRevealInExplorer: (worktreeId: string, absolutePath: string) => void
  onOpen: (event: React.MouseEvent<HTMLDivElement>) => void
  commentCount: number
  showPathHint?: boolean
}): React.JSX.Element {
  const FileIcon = getFileTypeIcon(entry.path)
  const fileName = basename(entry.path)
  const parentDir = dirname(entry.path)
  const dirPath = parentDir === '.' ? '' : parentDir

  return (
    <SourceControlEntryContextMenu
      currentWorktreeId={currentWorktreeId}
      absolutePath={joinPath(worktreePath, entry.path)}
      onRevealInExplorer={onRevealInExplorer}
    >
      <div
        className="group flex cursor-pointer items-center gap-1 pr-3 py-1 transition-colors hover:bg-accent/40"
        style={{
          paddingLeft: `${depth * SOURCE_CONTROL_TREE_INDENT_PX + SOURCE_CONTROL_TREE_FILE_PADDING_PX}px`
        }}
        draggable
        onDragStart={(e) => {
          const absolutePath = joinPath(worktreePath, entry.path)
          e.dataTransfer.setData(WORKSPACE_FILE_PATH_MIME, absolutePath)
          e.dataTransfer.effectAllowed = 'copy'
        }}
        onClick={onOpen}
      >
        <FileIcon className="size-3.5 shrink-0" style={{ color: STATUS_COLORS[entry.status] }} />
        <span className="min-w-0 flex-1 truncate text-xs">
          <span className="text-foreground">{fileName}</span>
          {showPathHint && dirPath && (
            <span className="ml-1.5 text-[11px] text-muted-foreground">{dirPath}</span>
          )}
        </span>
        {commentCount > 0 && (
          <span
            className="flex shrink-0 items-center gap-0.5 text-[10px] text-muted-foreground"
            title={`${commentCount} note${commentCount === 1 ? '' : 's'}`}
          >
            <MessageSquare className="size-3" />
            <span className="tabular-nums">{commentCount}</span>
          </span>
        )}
        <DiffLineCounts added={entry.added} removed={entry.removed} />
        <span
          className="w-4 shrink-0 text-center text-[10px] font-bold"
          style={{ color: STATUS_COLORS[entry.status] }}
        >
          {STATUS_LABELS[entry.status]}
        </span>
      </div>
    </SourceControlEntryContextMenu>
  )
}

export function SourceControlEntryContextMenu({
  currentWorktreeId,
  absolutePath,
  onRevealInExplorer,
  onOpenChange,
  children
}: {
  currentWorktreeId: string
  absolutePath?: string
  onRevealInExplorer: (worktreeId: string, absolutePath: string) => void
  onOpenChange?: (open: boolean) => void
  children: React.ReactNode
}): React.JSX.Element {
  const handleOpenInFileExplorer = useCallback(() => {
    if (!absolutePath) {
      return
    }
    onRevealInExplorer(currentWorktreeId, absolutePath)
  }, [absolutePath, currentWorktreeId, onRevealInExplorer])

  return (
    <ContextMenu onOpenChange={onOpenChange}>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent className="w-52">
        <ContextMenuItem onSelect={handleOpenInFileExplorer} disabled={!absolutePath}>
          <FolderOpen className="size-3.5" />
          Open in File Explorer
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}

export function ActionButton({
  icon: Icon,
  title,
  onClick,
  disabled
}: {
  icon: React.ComponentType<{ className?: string }>
  title: string
  onClick: (event: React.MouseEvent) => void
  disabled?: boolean
}): React.JSX.Element {
  // Why: use the Radix Tooltip instead of the native `title` attribute so the
  // label matches the rest of the sidebar chrome (consistent styling, no OS
  // delay quirks, dismissible on pointer leave).
  //
  // Why (no local TooltipProvider): the app root mounts a single
  // TooltipProvider (see App.tsx); nesting another one here gives this subtree
  // its own delay-timing state and breaks Radix's "skip the open delay when
  // moving between adjacent tooltip triggers" handoff between sibling action
  // buttons in the section header.
  //
  // Why (disabled handling): Radix's TooltipTrigger asChild on a disabled
  // <button> gets pointer-events blocked in Chromium, which suppresses the
  // tooltip entirely — a regression vs. the native `title` attribute it
  // replaced. We keep the button interactive and rely on the caller's
  // `isExecutingBulk` early-return to no-op the click during bulk ops;
  // `aria-disabled` + visual dimming preserves the disabled affordance.
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          className={cn(
            'text-muted-foreground hover:bg-background/70 hover:text-foreground',
            disabled && 'opacity-50 cursor-not-allowed'
          )}
          aria-label={title}
          aria-disabled={disabled}
          onClick={(event) => {
            if (disabled) {
              event.preventDefault()
              return
            }
            onClick(event)
          }}
        >
          <Icon className="size-3.5" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={6}>
        {title}
      </TooltipContent>
    </Tooltip>
  )
}
