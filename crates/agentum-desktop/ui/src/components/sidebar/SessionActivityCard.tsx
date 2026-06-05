import type React from 'react'
import { Sparkles, Wrench } from 'lucide-react'
import { useLatestAgentActivity } from './useLatestAgentActivity'

/** Expanded card shown under the currently-active session: last assistant
 *  message + last tool call. Pure re-layout of existing agent-status state —
 *  renders nothing until an agent reports activity for this worktree. */
export function SessionActivityCard({ worktreeId }: { worktreeId: string }): React.JSX.Element | null {
  const activity = useLatestAgentActivity(worktreeId)
  if (!activity.lastAssistantMessage && !activity.toolName) {
    return null
  }
  const toolLine = activity.toolName
    ? `${activity.toolName}${activity.toolInput ? ` ${activity.toolInput}` : ''}`
    : null
  return (
    <div className="mx-2 mb-1 rounded-lg border border-border/60 bg-card px-2.5 py-2 shadow-sm">
      {activity.lastAssistantMessage ? (
        <div className="flex items-start gap-1.5">
          <Sparkles className="mt-0.5 size-3 shrink-0 text-muted-foreground" />
          <span className="line-clamp-2 text-xs text-foreground">
            {activity.lastAssistantMessage}
          </span>
        </div>
      ) : null}
      {toolLine ? (
        <div className="mt-1 flex items-center gap-1.5">
          <Wrench className="size-3 shrink-0 text-muted-foreground" />
          <span className="truncate font-mono text-[11px] text-muted-foreground">{toolLine}</span>
        </div>
      ) : null}
    </div>
  )
}
