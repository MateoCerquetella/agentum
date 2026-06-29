import type { JSX } from 'react'
import { isOrchestrationSetupEnabled } from '@/lib/orchestration-setup-state'

// Agent Orchestration ships with agentum's MCP server — there is no skill to
// install. This card just explains the capability and reflects whether it's
// enabled; the real on/off switch is the server-side gate in
// Settings → Agent Orchestration.
export function OrchestrationSetupCard(props: { compact?: boolean }): JSX.Element {
  const enabled = isOrchestrationSetupEnabled()

  const card = (
    <div
      className={`flex flex-col gap-2 rounded-xl border border-border/60 bg-card/50 p-4 ${
        props.compact ? 'w-full max-w-[520px]' : ''
      }`}
    >
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm font-semibold">Agent Orchestration</p>
        <span
          className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${
            enabled
              ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400'
              : 'bg-muted text-muted-foreground'
          }`}
        >
          {enabled ? 'Enabled' : 'Off'}
        </span>
      </div>
      <p className="text-xs text-muted-foreground">
        Built into the agentum MCP — agents hand off context, message each other, and share tasks
        via <code className="text-foreground">agentum_send_message</code>,{' '}
        <code className="text-foreground">agentum_check_messages</code>, and the task tools. No skill
        to install.
      </p>
      {!enabled ? (
        <p className="text-[11px] text-muted-foreground">
          Turn it on in Settings → Agent Orchestration.
        </p>
      ) : null}
    </div>
  )

  if (props.compact) {
    return <div className="flex min-h-24 flex-1 items-center justify-center">{card}</div>
  }
  return <div className="flex">{card}</div>
}
