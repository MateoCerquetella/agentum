import { useEffect, useState } from 'react'
import {
  AlertTriangle,
  BellRing,
  CalendarClock,
  ExternalLink,
  FolderPlus,
  GitBranchPlus,
  type LucideIcon,
  Workflow
} from 'lucide-react'
import { api } from '@/tauri'
import { useAppStore } from '@/store'
import { getPreflightIssues, type PreflightIssue } from '@/lib/preflight-issues'
import { StatsPane } from '@/components/stats/StatsPane'
import { Badge } from '@/components/ui/badge'
import { isGitRepoKind } from '../../../../shared/repo-kind'
import {
  MISSION_CONTROL_SOON_CARDS,
  type MissionControlSoonCard
} from './mission-control-soon-cards'

const SOON_ICONS: Record<MissionControlSoonCard['icon'], LucideIcon> = {
  orchestration: Workflow,
  schedule: CalendarClock,
  cost: BellRing
}

function PreflightBanner({ issues }: { issues: PreflightIssue[] }): React.JSX.Element {
  return (
    <div className="w-full space-y-3 rounded-lg border border-yellow-500/30 bg-yellow-500/5 p-4">
      <div className="flex items-center gap-2 text-yellow-500">
        <AlertTriangle className="size-4 shrink-0" />
        <span className="text-sm font-medium">Missing dependencies</span>
      </div>
      <div className="space-y-2.5">
        {issues.map((issue) => (
          <div key={issue.id} className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-medium text-foreground">{issue.title}</p>
              <p className="mt-0.5 text-xs text-muted-foreground">{issue.description}</p>
            </div>
            <button
              className="inline-flex shrink-0 cursor-pointer items-center gap-1 text-xs font-medium text-blue-400 transition-colors hover:text-blue-300"
              onClick={() => api.shell.openUrl(issue.fixUrl)}
            >
              {issue.fixLabel}
              <ExternalLink className="size-3" />
            </button>
          </div>
        ))}
      </div>
    </div>
  )
}

export default function MissionControlPage(): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  const openModal = useAppStore((s) => s.openModal)
  const canCreateWorktree = repos.length > 0
  const createTargetLabel =
    canCreateWorktree && repos.every((repo) => isGitRepoKind(repo)) ? 'Worktree' : 'Workspace'

  const [preflightIssues, setPreflightIssues] = useState<PreflightIssue[]>([])

  useEffect(() => {
    let cancelled = false
    const refreshPreflight = (force = false): void => {
      void api.preflight.check(force ? { force: true } : undefined).then((status) => {
        if (cancelled) {
          return
        }
        setPreflightIssues(getPreflightIssues(status))
      })
    }

    refreshPreflight()

    // Why: users often install/authenticate gh outside Agentum. Re-check when the
    // window becomes active again so the warning clears without relaunch.
    const handleWindowActive = (): void => {
      if (document.visibilityState === 'visible') {
        refreshPreflight(true)
      }
    }

    document.addEventListener('visibilitychange', handleWindowActive)
    window.addEventListener('focus', handleWindowActive)

    return () => {
      cancelled = true
      document.removeEventListener('visibilitychange', handleWindowActive)
      window.removeEventListener('focus', handleWindowActive)
    }
  }, [])

  useEffect(() => {
    if (preflightIssues.length === 0) {
      return
    }
    let cancelled = false
    // Why: some users complete `gh auth login` without leaving the window. Poll
    // only while a warning is visible so the banner self-clears.
    const intervalId = window.setInterval(() => {
      void api.preflight.check({ force: true }).then((status) => {
        if (cancelled) {
          return
        }
        setPreflightIssues(getPreflightIssues(status))
      })
    }, 30000)
    return () => {
      cancelled = true
      window.clearInterval(intervalId)
    }
  }, [preflightIssues.length])

  return (
    <div className="flex h-full flex-col overflow-hidden bg-background">
      <header className="flex items-center justify-between gap-3 border-b border-border/60 px-6 py-3">
        <div className="min-w-0">
          <h1 className="text-sm font-semibold text-foreground">Mission Control</h1>
          <p className="text-xs text-muted-foreground">
            Usage, cost, and agent activity at a glance.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button
            className="inline-flex items-center gap-1.5 rounded-md border border-border/80 bg-secondary/70 px-3 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-accent"
            onClick={() => openModal('add-repo')}
          >
            <FolderPlus className="size-3.5" />
            Add Project
          </button>
          <button
            className="inline-flex items-center gap-1.5 rounded-md border border-border/80 bg-secondary/70 px-3 py-1.5 text-sm font-medium text-foreground transition-colors enabled:cursor-pointer enabled:hover:bg-accent disabled:cursor-not-allowed disabled:opacity-40"
            disabled={!canCreateWorktree}
            title={!canCreateWorktree ? 'Add a project first' : undefined}
            onClick={() => openModal('new-workspace-composer', { telemetrySource: 'unknown' })}
          >
            <GitBranchPlus className="size-3.5" />
            Create {createTargetLabel}
          </button>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="w-full space-y-6 p-6 md:px-8">
          {preflightIssues.length > 0 && <PreflightBanner issues={preflightIssues} />}

          <StatsPane />

          <section className="space-y-3">
            <h2 className="text-sm font-semibold text-foreground">Coming soon</h2>
            <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
              {MISSION_CONTROL_SOON_CARDS.map((card) => {
                const Icon = SOON_ICONS[card.icon]
                return (
                  <div
                    key={card.id}
                    className="cursor-default rounded-lg border border-dashed border-border/60 bg-card/30 p-4 opacity-80"
                  >
                    <div className="mb-2 flex items-center justify-between gap-2">
                      <span className="inline-flex size-8 items-center justify-center rounded-md border border-border/60 bg-card/60 text-muted-foreground">
                        <Icon className="size-4" />
                      </span>
                      <Badge variant="outline" className="shrink-0">
                        Soon
                      </Badge>
                    </div>
                    <h3 className="text-sm font-semibold text-foreground">{card.title}</h3>
                    <p className="mt-1 text-xs text-muted-foreground">{card.description}</p>
                  </div>
                )
              })}
            </div>
          </section>
        </div>
      </div>
    </div>
  )
}
