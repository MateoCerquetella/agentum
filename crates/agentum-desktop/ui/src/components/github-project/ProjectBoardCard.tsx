// A compact Kanban card for a GitHub Project row, used by ProjectBoardView.
// Mirrors ProjectRow's affordances (open-in-GitHub, start-work, draft handling)
// in a card form. Purely presentational — the board owns drag state and moves.
import React from 'react'
import { CircleDot, EyeOff, ExternalLink, GitPullRequest, Play, SquarePen } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { GitHubProjectRow } from '../../../../shared/github-project-types'

// GitHub label colors come back as 6-hex without a `#`; normalize for CSS.
function labelHex(color: string): string {
  if (!color) {
    return '#8b949e'
  }
  if (color.startsWith('#')) {
    return color
  }
  return /^[0-9a-fA-F]{6}$/.test(color) ? `#${color}` : color
}

type Props = {
  row: GitHubProjectRow
  draggable: boolean
  onDragStart: (e: React.DragEvent) => void
  onOpenDialog?: () => void
  onOpenInBrowser?: () => void
  onStartWork?: () => void
}

export default function ProjectBoardCard({
  row,
  draggable,
  onDragStart,
  onOpenDialog,
  onOpenInBrowser,
  onStartWork
}: Props): React.JSX.Element {
  const { content } = row
  const TypeIcon =
    row.itemType === 'PULL_REQUEST'
      ? GitPullRequest
      : row.itemType === 'DRAFT_ISSUE'
        ? SquarePen
        : row.itemType === 'REDACTED'
          ? EyeOff
          : CircleDot
  // Drafts/redacted rows have no detail page to open; keep their title static.
  const titleClickable = Boolean(onOpenDialog) && row.itemType !== 'REDACTED'
  const canStartWork =
    row.itemType !== 'REDACTED' && row.itemType !== 'DRAFT_ISSUE' && content.number != null
  const labels = content.labels.slice(0, 4)
  const assignees = content.assignees.slice(0, 3)

  return (
    <div
      draggable={draggable}
      onDragStart={draggable ? onDragStart : undefined}
      className={cn(
        'group rounded-md border border-border bg-background p-2.5 shadow-sm',
        draggable
          ? 'cursor-grab active:cursor-grabbing hover:border-foreground/30'
          : 'cursor-default',
        row.itemType === 'REDACTED' && 'opacity-60'
      )}
    >
      <div className="mb-1 flex items-center gap-1.5">
        <TypeIcon className="size-3.5 flex-none text-muted-foreground" />
        {content.number != null ? (
          <span className="font-mono text-[10.5px] text-muted-foreground">#{content.number}</span>
        ) : null}
        <div className="ml-auto flex items-center gap-0.5 opacity-0 transition group-hover:opacity-100">
          {content.url && onOpenInBrowser ? (
            <button
              type="button"
              onClick={onOpenInBrowser}
              aria-label="Open in GitHub"
              className="rounded p-1 hover:bg-muted"
            >
              <ExternalLink className="size-3.5" />
            </button>
          ) : null}
          {canStartWork && onStartWork ? (
            <button
              type="button"
              onClick={onStartWork}
              aria-label="Start work"
              className="rounded p-1 hover:bg-muted"
            >
              <Play className="size-3.5" />
            </button>
          ) : null}
        </div>
      </div>

      {titleClickable ? (
        <button type="button" onClick={onOpenDialog} className="block w-full text-left">
          <span className="line-clamp-3 text-[12.5px] leading-snug hover:underline">
            {content.title}
          </span>
        </button>
      ) : (
        <span className="line-clamp-3 text-[12.5px] leading-snug">{content.title}</span>
      )}

      {content.repository ? (
        <div className="mt-1 truncate font-mono text-[10px] text-muted-foreground">
          {content.repository}
        </div>
      ) : null}

      {labels.length > 0 || assignees.length > 0 ? (
        <div className="mt-2 flex items-center gap-1">
          <div className="flex min-w-0 flex-wrap gap-1">
            {labels.map((l) => (
              <span
                key={l.name}
                className="max-w-[110px] truncate rounded-full px-1.5 py-px text-[9.5px] leading-normal"
                style={{
                  backgroundColor: `${labelHex(l.color)}22`,
                  boxShadow: `inset 0 0 0 1px ${labelHex(l.color)}66`
                }}
              >
                {l.name}
              </span>
            ))}
          </div>
          {assignees.length > 0 ? (
            <div className="ml-auto flex flex-none -space-x-1.5">
              {assignees.map((a) =>
                a.avatarUrl ? (
                  <img
                    key={a.login}
                    src={a.avatarUrl}
                    alt={a.login}
                    title={a.login}
                    className="size-4 rounded-full ring-1 ring-background"
                  />
                ) : (
                  <span
                    key={a.login}
                    title={a.login}
                    className="flex size-4 items-center justify-center rounded-full bg-muted text-[8px] ring-1 ring-background"
                  >
                    {a.login.slice(0, 1).toUpperCase()}
                  </span>
                )
              )}
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
