import type { JSX } from 'react'
import { BROWSER_USE_ENABLED_STORAGE_KEY } from '@/lib/browser-use-setup-state'

// Browser control ships with agentum's MCP server (`agentum_browser`) — there is
// no skill to install. This card explains the capability and reflects whether the
// user has turned it on; logins are imported in Settings → Browser Use.
export function BrowserUseSkillSetupCard(props: { compact?: boolean }): JSX.Element {
  const enabled = localStorage.getItem(BROWSER_USE_ENABLED_STORAGE_KEY) === '1'

  const card = (
    <div
      className={`flex flex-col gap-2 rounded-xl border border-border/60 bg-card/50 p-4 ${
        props.compact ? 'w-full max-w-[520px]' : ''
      }`}
    >
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm font-semibold">Agent Browser Use</p>
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
        Built into the agentum MCP — agents drive the browser with the{' '}
        <code className="text-foreground">agentum_browser</code> tool. No skill to install.
      </p>
      <p className="text-[11px] text-muted-foreground">
        Import your logins in Settings → Browser Use so agents can reach authenticated pages.
      </p>
    </div>
  )

  if (props.compact) {
    return <div className="flex min-h-24 flex-1 items-center justify-center pt-3">{card}</div>
  }
  return <div className="flex">{card}</div>
}
