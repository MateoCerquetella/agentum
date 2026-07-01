import type { ReactNode } from 'react'
import { Globe2, MonitorCog, Workflow } from 'lucide-react'
import { cn } from '@/lib/utils'
import type {
  OnboardingFeatureSetupId,
  OnboardingFeatureSetupSelection
} from './onboarding-feature-setup'

type FeatureSetupChecklistProps = {
  value: OnboardingFeatureSetupSelection
  onChange: (value: OnboardingFeatureSetupSelection) => void
}

type FeatureSetupRow = {
  id: OnboardingFeatureSetupId
  title: string
  description: string
  icon: ReactNode
}

// Each row is an agentum MCP tool, not a "skill" to install: agentum wires its
// own MCP server into every agent it launches, and these switches pick which of
// its tools those agents get. Mirrors the Settings "Agent MCP" panes so the two
// surfaces read the same way.
const FEATURE_SETUP_ROWS: readonly FeatureSetupRow[] = [
  {
    id: 'browserUse',
    title: 'Agent Browser Use',
    description:
      'Agents drive and inspect web pages with the agentum_browser tool. Add your logins later in Settings.',
    icon: <Globe2 className="size-4" />
  },
  {
    id: 'computerUse',
    title: 'Computer Use',
    description:
      'Agents inspect windows and operate local apps with the agentum_computer tool. Grant the macOS permissions when prompted.',
    icon: <MonitorCog className="size-4" />
  },
  {
    id: 'orchestration',
    title: 'Agent Orchestration',
    description:
      'Agents message each other, take tasks, and coordinate handoffs through the orchestration tools.',
    icon: <Workflow className="size-4" />
  }
]

export function FeatureSetupChecklist({
  value,
  onChange
}: FeatureSetupChecklistProps): React.JSX.Element {
  return (
    <section className="mt-6 space-y-3">
      <div className="space-y-0.5">
        <p className="text-sm font-medium text-foreground">Agent MCP tools</p>
        <p className="text-xs leading-relaxed text-muted-foreground">
          agentum wires its own MCP into every agent it launches. Choose which tools they get — you
          can toggle any of these later in Settings.
        </p>
      </div>
      <div className="divide-y divide-border/60 overflow-hidden rounded-lg border border-border/60 bg-muted/10">
        {FEATURE_SETUP_ROWS.map((row) => {
          const enabled = value[row.id]
          return (
            <div key={row.id} className="flex items-start justify-between gap-4 px-4 py-3">
              <div className="flex min-w-0 items-start gap-3">
                <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg border border-border bg-muted/40 text-foreground">
                  {row.icon}
                </span>
                <div className="min-w-0 space-y-0.5">
                  <p className="text-sm font-medium text-foreground">{row.title}</p>
                  <p className="text-xs leading-relaxed text-muted-foreground">{row.description}</p>
                </div>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={enabled}
                aria-label={row.title}
                onClick={() => onChange({ ...value, [row.id]: !enabled })}
                className={cn(
                  'relative mt-0.5 inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors',
                  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
                  enabled ? 'bg-foreground' : 'bg-muted-foreground/30'
                )}
              >
                <span
                  className={cn(
                    'inline-block h-3.5 w-3.5 transform rounded-full bg-background shadow-sm transition-transform',
                    enabled ? 'translate-x-4' : 'translate-x-0.5'
                  )}
                />
              </button>
            </div>
          )
        })}
      </div>
    </section>
  )
}
