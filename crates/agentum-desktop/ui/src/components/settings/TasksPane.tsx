import { Check, Github, Gitlab } from 'lucide-react'
import type { ChatAgentId, GlobalSettings, TaskProvider } from '@/shared/types'
import {
  TASK_PROVIDERS,
  normalizeVisibleTaskProviders,
  resolveVisibleTaskProvider
} from '@/shared/task-providers'
import { cn } from '@/lib/utils'
import { LinearIcon } from '@/components/icons/LinearIcon'
import { Label } from '../ui/label'
import { SearchableSetting } from './SearchableSetting'
import { SettingsSubsectionHeader } from './SettingsFormControls'
import { useDetectedAgents } from '@/hooks/useDetectedAgents'
import { AgentIcon } from '@/lib/agent-catalog'
import { CHAT_AGENTS, pickChatAgent } from '@/runtime/chat-client'

type TasksPaneProps = {
  settings: GlobalSettings
  updateSettings: (updates: Partial<GlobalSettings>) => void
}

const TASK_PROVIDER_OPTIONS: readonly {
  id: TaskProvider
  label: string
  description: string
  Icon: (props: { className?: string }) => React.JSX.Element
}[] = [
  {
    id: 'github',
    label: 'GitHub',
    description: 'Show GitHub in the Tasks source picker and sidebar shortcuts.',
    Icon: ({ className }) => <Github className={className} />
  },
  {
    id: 'gitlab',
    label: 'GitLab',
    description: 'Show GitLab in the Tasks source picker and sidebar shortcuts.',
    Icon: ({ className }) => <Gitlab className={className} />
  },
  {
    id: 'linear',
    label: 'Linear',
    description: 'Show Linear in the Tasks source picker and sidebar shortcuts.',
    Icon: ({ className }) => <LinearIcon className={className} />
  }
]

export function TasksPane({ settings, updateSettings }: TasksPaneProps): React.JSX.Element {
  const visibleProviders = normalizeVisibleTaskProviders(settings.visibleTaskProviders)
  const { detectedIds, isLoading } = useDetectedAgents()
  const installedChatAgents = CHAT_AGENTS.filter((agent) => detectedIds?.includes(agent.id))
  const selectedChatAgent = pickChatAgent(settings.chatAgent, detectedIds)

  const toggleProvider = (provider: TaskProvider): void => {
    const isVisible = visibleProviders.includes(provider)
    if (isVisible && visibleProviders.length === 1) {
      return
    }

    const nextProviders = isVisible
      ? visibleProviders.filter((entry) => entry !== provider)
      : TASK_PROVIDERS.filter((entry) => entry === provider || visibleProviders.includes(entry))

    updateSettings({
      visibleTaskProviders: nextProviders,
      defaultTaskSource: resolveVisibleTaskProvider(settings.defaultTaskSource, nextProviders)
    })
  }

  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <SettingsSubsectionHeader
          title="Issue drafting agent"
          description="Choose the installed agent used to draft optional tracker issue descriptions."
        />

        <SearchableSetting
          title="Drafting agent"
          description="The selected agent is saved globally and uses its existing sign-in or API key."
          keywords={['draft', 'agent', 'claude', 'codex', 'issues']}
          className="flex flex-wrap gap-2 py-2"
        >
          {installedChatAgents.map((agent) => {
            const active = selectedChatAgent === agent.id
            return (
              <button
                key={agent.id}
                type="button"
                aria-pressed={active}
                onClick={() => updateSettings({ chatAgent: agent.id as ChatAgentId })}
                className={cn(
                  'inline-flex items-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors',
                  active
                    ? 'border-muted-foreground/40 bg-accent font-medium text-accent-foreground'
                    : 'border-border bg-background/50 text-muted-foreground hover:text-foreground'
                )}
              >
                <AgentIcon agent={agent.id} size={14} />
                {agent.label}
                {active ? <Check className="size-3.5" /> : null}
              </button>
            )
          })}
          {isLoading ? (
            <span className="text-xs text-muted-foreground">Detecting installed agents…</span>
          ) : installedChatAgents.length === 0 ? (
            <span className="text-xs text-destructive">
              Install Claude or Codex, then refresh agent detection in Settings → Agents.
            </span>
          ) : null}
        </SearchableSetting>
      </section>

      <section className="space-y-3">
        <SettingsSubsectionHeader
          title="Task Sources"
          description="Choose which task providers appear in the Tasks page source picker and sidebar shortcuts. At least one provider must stay visible."
        />

        <SearchableSetting
          title="Task Providers"
          description="Choose which task providers appear in the Tasks page and sidebar shortcuts."
          keywords={[
            'tasks',
            'provider',
            'source',
            'github',
            'gitlab',
            'linear',
            'display',
            'hide'
          ]}
          className="grid gap-2 py-2"
        >
          {TASK_PROVIDER_OPTIONS.map((option) => {
            const enabled = visibleProviders.includes(option.id)
            const isLastEnabled = enabled && visibleProviders.length === 1
            const Icon = option.Icon

            return (
              <button
                key={option.id}
                type="button"
                role="checkbox"
                aria-checked={enabled}
                aria-disabled={isLastEnabled}
                onClick={() => toggleProvider(option.id)}
                className={cn(
                  'flex w-full items-center gap-3 rounded-md border border-border/60 px-3 py-2.5 text-left transition-colors',
                  enabled
                    ? 'bg-accent/70 text-accent-foreground'
                    : 'bg-transparent hover:bg-muted/50',
                  isLastEnabled && 'cursor-not-allowed'
                )}
              >
                <span
                  className={cn(
                    'flex size-7 shrink-0 items-center justify-center rounded-md border',
                    enabled
                      ? 'border-foreground/20 bg-background/70'
                      : 'border-border/60 bg-muted/40 text-muted-foreground'
                  )}
                >
                  <Icon className="size-3.5" />
                </span>
                <span className="min-w-0 flex-1 space-y-0.5">
                  <Label className="cursor-inherit">{option.label}</Label>
                  <span className="block text-xs text-muted-foreground">{option.description}</span>
                </span>
                <span
                  aria-hidden
                  className={cn(
                    'flex size-4 shrink-0 items-center justify-center rounded border text-[10px]',
                    enabled
                      ? 'border-foreground/50 bg-foreground text-background'
                      : 'border-border bg-background'
                  )}
                >
                  {enabled ? <Check className="size-3" /> : null}
                </span>
              </button>
            )
          })}
        </SearchableSetting>
      </section>
    </div>
  )
}
