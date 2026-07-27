/* eslint-disable max-lines -- Why: the card metadata hover keeps compact badge rendering,
   provider-specific action rows, and markdown note preview together so the sidebar
   card has one metadata contract. */
import React from 'react'
import { useLatestAgentActivity } from './useLatestAgentActivity'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { HoverCard, HoverCardTrigger, HoverCardContent } from '@/components/ui/hover-card'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { AlertTriangle, CircleDot, ExternalLink, MonitorUp, Pencil, StickyNote } from 'lucide-react'
import { cn } from '@/lib/utils'
import { api } from '@/tauri'
import { LinearIcon } from '@/components/icons/LinearIcon'
import { SelectedTextCopyMenu } from '@/components/SelectedTextCopyMenu'
import CommentMarkdown from './CommentMarkdown'
import { WORKTREE_NATIVE_CONTEXT_MENU_ATTR } from './WorktreeContextMenu'
import {
  WorktreeCardDetailSection,
  WorktreeCardDetailSectionContent
} from './WorktreeCardDetailSection'
import { LinearStateBadge } from './WorktreeCardMetadataStatusBadges'
import { IssueProjectStatusChip, useIssueProjectStatus } from './IssueProjectStatusChip'
import type { IssueInfo } from '@/shared/types'

export type WorktreeCardIssueDisplay =
  | IssueInfo
  | {
      number: number
      title: string
      state?: IssueInfo['state']
      url?: string
      labels?: string[]
    }

type WorktreeCardLinearIssueDisplay = {
  identifier: string
  title: string
  url?: string
  stateName?: string
  labels?: string[]
}

type WorktreeCardMetaBadgesProps = {
  issue: WorktreeCardIssueDisplay | null
  linearIssue: WorktreeCardLinearIssueDisplay | null
  comment: string | null
}

type WorktreeCardMetaBadgesRootProps = WorktreeCardMetaBadgesProps &
  React.HTMLAttributes<HTMLDivElement>

type WorktreeCardDetailsHoverProps = WorktreeCardMetaBadgesProps & {
  children: React.ReactElement
  branchName?: string
  workspaceTitle?: string
  detailsAfter?: React.ReactNode
  /** Worktree id lets a confirmed GitHub option reconcile the local cache. */
  worktreeId?: string
  /** Spec 018 (#365): the repo the linked issue lives in, for the on-open
   *  Project-status read (binding lookup). Optional — no chip without them. */
  workdir?: string
  repoId?: string
  onEditIssue: (event: React.MouseEvent) => void
  onEditComment: (event: React.MouseEvent) => void
  onOpenGitHubIssueInAgentum?: (event: React.MouseEvent) => void
  onOpenLinearIssueInAgentum?: (event: React.MouseEvent) => void
}

// Keep synchronized with the Agentum-managed lifecycle labels in
// crates/agentum-server/src/task_sink.rs. Human QA labels are intentionally
// excluded so they remain visible beside an authoritative Project status.
const AGENTUM_TRACKER_LABELS: ReadonlySet<string> = new Set([
  'status/todo',
  'status/in-progress',
  'status/in-review',
  'status/ready-to-test',
  'status/done',
  'status/blocked'
])

export function visibleIssueLabels(
  labels: readonly string[],
  projectStatus: string | null | undefined
): readonly string[] {
  if (!projectStatus?.trim()) {
    return labels
  }

  return labels.filter((label) => !AGENTUM_TRACKER_LABELS.has(label))
}

function hasComment(comment: string | null): boolean {
  return (comment ?? '').trim().length > 0
}

export function hasWorktreeCardDetails({
  issue,
  linearIssue,
  comment
}: WorktreeCardMetaBadgesProps): boolean {
  return Boolean(issue || linearIssue || hasComment(comment))
}

function MetaIconBadge({
  label,
  children
}: {
  label: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <span className="inline-flex size-3.5 shrink-0 items-center justify-center text-muted-foreground/70 hover:text-foreground [&>svg]:size-3.5">
      {children}
      <span className="sr-only">{label}</span>
    </span>
  )
}

function DetailHeader({
  icon,
  label,
  actions
}: {
  icon: React.ReactNode
  label: string
  actions?: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="flex items-center justify-between gap-2">
      <div className="flex min-w-0 items-center gap-1.5 text-[11px] font-semibold uppercase tracking-[0.05em] text-muted-foreground">
        {icon}
        <span className="truncate">{label}</span>
      </div>
      {actions && <div className="flex shrink-0 items-center gap-0.5">{actions}</div>}
    </div>
  )
}

function MetadataActionIcon({
  label,
  onClick,
  children
}: {
  label: string
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void
  children: React.ReactNode
}): React.JSX.Element {
  // Always a real <button>. An <a target="_blank"> is dead inside the Tauri
  // webview (no window-open handler), so external links route through
  // api.shell.openUrl via onClick — never an anchor.
  const trigger = (
    <Button
      type="button"
      variant="ghost"
      size="icon-xs"
      className="size-6"
      aria-label={label}
      onClick={(event) => {
        event.stopPropagation()
        onClick?.(event)
      }}
    >
      {children}
    </Button>
  )

  return (
    <Tooltip>
      <TooltipTrigger asChild>{trigger}</TooltipTrigger>
      <TooltipContent side="top" sideOffset={4}>
        {label}
      </TooltipContent>
    </Tooltip>
  )
}

export const WorktreeCardMetaBadges = React.forwardRef<
  HTMLDivElement,
  WorktreeCardMetaBadgesRootProps
>(function WorktreeCardMetaBadges(
  { issue, linearIssue, comment, className, ...props },
  ref
): React.JSX.Element | null {
  if (!hasWorktreeCardDetails({ issue, linearIssue, comment })) {
    return null
  }

  return (
    // Why: Radix HoverCardTrigger uses `asChild`, so this group must forward
    // trigger props/ref to the actual DOM node for attachment-only hover.
    <div
      ref={ref}
      {...props}
      className={cn('ml-auto flex shrink-0 items-center gap-1 pr-1.5', className)}
      aria-label="Workspace metadata"
    >
      {hasComment(comment) && (
        <MetaIconBadge label="Workspace notes">
          <StickyNote className="text-muted-foreground" />
        </MetaIconBadge>
      )}
      {issue && (
        <MetaIconBadge label={`Linked issue #${issue.number}`}>
          <CircleDot className="text-muted-foreground" />
        </MetaIconBadge>
      )}
      {linearIssue && (
        <MetaIconBadge label={`Linked Linear ${linearIssue.identifier}`}>
          <LinearIcon className="text-muted-foreground" />
        </MetaIconBadge>
      )}
    </div>
  )
})

/** Muted "ctx N%" chip shown on the worktree leaf when an agent reports context usage. */
export function WorktreeCardCtxChip({
  worktreeId
}: {
  worktreeId: string
}): React.JSX.Element | null {
  const { contextUsagePercent } = useLatestAgentActivity(worktreeId)
  if (typeof contextUsagePercent !== 'number') {
    return null
  }
  return (
    <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
      ctx {Math.round(contextUsagePercent)}%
    </span>
  )
}

export function WorktreeCardDetailsHover({
  issue,
  linearIssue,
  comment,
  children,
  branchName,
  workspaceTitle,
  detailsAfter,
  worktreeId,
  workdir,
  repoId,
  onEditIssue,
  onEditComment,
  onOpenGitHubIssueInAgentum,
  onOpenLinearIssueInAgentum
}: WorktreeCardDetailsHoverProps): React.JSX.Element {
  const [open, setOpen] = React.useState(false)
  // #379 perf: `open: true` prefetches the Board status at card RENDER (not
  // first hover), so the hover paints it instantly; the shared TTL cache +
  // bus-event invalidation keep the cost to one fetch per issue per window.
  const projectStatus = useIssueProjectStatus({
    open: true,
    issueUrl: issue?.url,
    workdir,
    repoId,
    worktreeId
  })
  const dismissAndRun = React.useCallback(
    (handler: ((event: React.MouseEvent) => void) | undefined) => (event: React.MouseEvent) => {
      setOpen(false)
      handler?.(event)
    },
    []
  )

  const showIdentityHeader = Boolean(branchName || workspaceTitle)

  if (
    !showIdentityHeader &&
    !hasWorktreeCardDetails({ issue, linearIssue, comment }) &&
    !detailsAfter
  ) {
    return children
  }

  const issueLabels = issue?.labels ?? []
  const displayedIssueLabels = visibleIssueLabels(issueLabels, projectStatus.status)

  return (
    <HoverCard open={open} onOpenChange={setOpen} openDelay={250} closeDelay={120}>
      <HoverCardTrigger asChild>{children}</HoverCardTrigger>
      <HoverCardContent
        side="right"
        align="start"
        sideOffset={8}
        className="w-80 max-h-[28rem] overflow-y-auto p-3 text-xs scrollbar-sleek"
        {...{ [WORKTREE_NATIVE_CONTEXT_MENU_ATTR]: '' }}
        onClick={(event) => event.stopPropagation()}
        onDoubleClick={(event) => event.stopPropagation()}
      >
        <SelectedTextCopyMenu className="space-y-3">
          {showIdentityHeader && (
            <div className="min-w-0 border-l border-border/70 pl-2">
              {/* Why: the closed card no longer carries a branch row; custom-titled
                  worktrees still need their git branch available in the hover. */}
              {branchName && (
                <div className="truncate font-mono text-[11px] leading-none text-muted-foreground">
                  {branchName}
                </div>
              )}
              {workspaceTitle && workspaceTitle !== branchName && (
                <div className="mt-1 truncate text-[13px] font-semibold leading-snug text-foreground">
                  {workspaceTitle}
                </div>
              )}
            </div>
          )}

          {issue && (
            <WorktreeCardDetailSection>
              <DetailHeader
                icon={<CircleDot className="size-3 text-muted-foreground" />}
                label={`Issue #${issue.number}`}
                actions={
                  <>
                    {issue.url && onOpenGitHubIssueInAgentum && (
                      <MetadataActionIcon
                        label="Open in Agentum"
                        onClick={dismissAndRun(onOpenGitHubIssueInAgentum)}
                      >
                        <MonitorUp className="size-3" />
                      </MetadataActionIcon>
                    )}
                    {issue.url && (
                      <MetadataActionIcon
                        label="View on GitHub"
                        onClick={() => void api.shell.openUrl(issue.url!)}
                      >
                        <ExternalLink className="size-3" />
                      </MetadataActionIcon>
                    )}
                    <MetadataActionIcon label="Edit issue" onClick={onEditIssue}>
                      <Pencil className="size-3" />
                    </MetadataActionIcon>
                  </>
                }
              />
              <WorktreeCardDetailSectionContent className="space-y-1.5">
                <div className="text-[13px] font-semibold leading-snug text-foreground break-words">
                  {issue.title}
                </div>
                {(displayedIssueLabels.length > 0 || projectStatus.status) && (
                  <div className="flex flex-wrap gap-1">
                    {/* #379 (Mateo): the hover shows the BOARD status + the
                        labels only — the open/closed state badge was noise
                        (state is implied by the board column + labels). */}
                    {/* #399: GitHub Project Status is the sole lifecycle chip.
                        Never render the local TrackerPhaseChip beside it. */}
                    <IssueProjectStatusChip status={projectStatus.status} />
                    {displayedIssueLabels.map((label) => (
                      <Badge key={label} variant="outline" className="h-4 px-1.5 text-[9px]">
                        {label}
                      </Badge>
                    ))}
                  </div>
                )}
                {projectStatus.warning && (
                  <div
                    role="status"
                    className="flex gap-1.5 rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1.5 text-[10px] leading-snug text-amber-700 dark:text-amber-300"
                  >
                    <AlertTriangle className="mt-0.5 size-3 shrink-0" />
                    <span>{projectStatus.warning}</span>
                  </div>
                )}
              </WorktreeCardDetailSectionContent>
            </WorktreeCardDetailSection>
          )}

          {linearIssue && (
            <WorktreeCardDetailSection>
              <DetailHeader
                icon={<LinearIcon className="size-3 text-muted-foreground" />}
                label={`Linear ${linearIssue.identifier}`}
                actions={
                  <>
                    {linearIssue.url && onOpenLinearIssueInAgentum && (
                      <MetadataActionIcon
                        label="Open in Agentum"
                        onClick={dismissAndRun(onOpenLinearIssueInAgentum)}
                      >
                        <MonitorUp className="size-3" />
                      </MetadataActionIcon>
                    )}
                    {linearIssue.url && (
                      <MetadataActionIcon
                        label="View on Linear"
                        onClick={() => void api.shell.openUrl(linearIssue.url!)}
                      >
                        <ExternalLink className="size-3" />
                      </MetadataActionIcon>
                    )}
                  </>
                }
              />
              <WorktreeCardDetailSectionContent className="space-y-1.5">
                <div className="text-[13px] font-semibold leading-snug text-foreground break-words">
                  {linearIssue.title}
                </div>
                {((linearIssue.labels && linearIssue.labels.length > 0) ||
                  linearIssue.stateName) && (
                  <div className="flex flex-wrap gap-1">
                    {linearIssue.stateName && (
                      <LinearStateBadge stateName={linearIssue.stateName} />
                    )}
                    {(linearIssue.labels ?? []).map((label) => (
                      <Badge key={label} variant="outline" className="h-4 px-1.5 text-[9px]">
                        {label}
                      </Badge>
                    ))}
                  </div>
                )}
              </WorktreeCardDetailSectionContent>
            </WorktreeCardDetailSection>
          )}

          {hasComment(comment) && (
            <WorktreeCardDetailSection>
              <DetailHeader
                icon={<StickyNote className="size-3 text-muted-foreground" />}
                label="Notes"
                actions={
                  <MetadataActionIcon label="Edit notes" onClick={onEditComment}>
                    <Pencil className="size-3" />
                  </MetadataActionIcon>
                }
              />
              <WorktreeCardDetailSectionContent className="space-y-2">
                <CommentMarkdown
                  content={comment ?? ''}
                  className="text-[11.5px] text-foreground break-words leading-normal [&_.comment-md-p]:block [&_.comment-md-p+.comment-md-p]:mt-1"
                />
              </WorktreeCardDetailSectionContent>
            </WorktreeCardDetailSection>
          )}

          {detailsAfter}
        </SelectedTextCopyMenu>
      </HoverCardContent>
    </HoverCard>
  )
}
