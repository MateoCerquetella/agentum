import { type ReactNode } from 'react'
import { cn } from '@/lib/utils'

type AgentSkillSetupPanelVariant = 'card' | 'inline'

// The full prop surface is preserved so existing call sites compile unchanged —
// most fields are now ignored (see below).
type AgentSkillSetupPanelProps = {
  title: string
  description: ReactNode
  command: string
  terminalTitle: string
  terminalAriaLabel: string
  terminalWorktreeId: string
  installed: boolean
  loading: boolean
  error: string | null
  installDisabled?: boolean
  terminalHeightPx?: number
  leading?: ReactNode
  icon?: ReactNode
  variant?: AgentSkillSetupPanelVariant
  className?: string
  showRecheckWhenInstalled?: boolean
  onRecheck: () => void | Promise<void>
}

// Skill installation is retired. agentum exposes its capabilities as MCP tools
// wired into every agent at launch (agentum-server `routes/mcp.rs` +
// `mcp_provision.rs`), so there is no per-agent skill to install. This widget
// used to render an `npx skills add …` install terminal; it now renders a short
// note. The props are kept intact so every call site (the feature panes and the
// onboarding flow) compiles without change.
export function AgentSkillSetupPanel({
  title,
  icon,
  variant = 'card',
  className
}: AgentSkillSetupPanelProps): React.JSX.Element {
  return (
    <div
      className={cn(
        variant === 'card' ? 'rounded-lg border border-border/60 p-3' : 'mt-3',
        className
      )}
    >
      <div className="flex items-center gap-2 text-sm font-medium">
        {icon}
        <span>{title}</span>
      </div>
      <p className="mt-1 text-xs text-muted-foreground">
        Provided automatically by agentum&apos;s MCP server — no install needed.
      </p>
    </div>
  )
}
