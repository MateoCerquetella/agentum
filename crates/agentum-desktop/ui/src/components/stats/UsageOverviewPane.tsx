/* eslint-disable max-lines -- Why: the overview keeps its small display components beside the
   provider fetch wiring so the combined usage surface stays easy to audit. */
import { useEffect, useMemo } from 'react'
import {
  Activity,
  AlertCircle,
  CalendarDays,
  Coins,
  DatabaseZap,
  RefreshCw,
  Sparkles
} from 'lucide-react'
import { useAppStore } from '../../store'
import { Badge } from '../ui/badge'
import { Button } from '../ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '../ui/tooltip'
import { StatCard } from './StatCard'
import {
  buildUsageOverview,
  formatUsageCost,
  formatUsageTokens,
  getRecentUsageDays,
  type UsageOverviewDailyPoint,
  type UsageOverviewModel,
  type UsageProviderOverview
} from './usage-overview-model'

const RECENT_DAY_COUNT = 42

const INTENSITY_CLASS: Record<UsageOverviewDailyPoint['intensity'], string> = {
  0: 'border-border/50 bg-muted/30',
  1: 'border-emerald-500/40 bg-emerald-500/50',
  2: 'border-amber-500/40 bg-amber-400/65',
  3: 'border-orange-500/45 bg-orange-500/75',
  4: 'border-rose-500/50 bg-rose-500/85'
}

function formatPercent(value: number | null): string {
  if (value === null) {
    return 'n/a'
  }
  return `${Math.round(value * 100)}%`
}

function formatUpdatedAt(timestamp: number | null): string {
  if (!timestamp) {
    return 'Not scanned yet'
  }
  return `Updated ${new Date(timestamp).toLocaleString()}`
}

function formatDayLabel(day: string): string {
  const parsed = new Date(`${day}T12:00:00`)
  if (Number.isNaN(parsed.getTime())) {
    return day
  }
  return parsed.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}

function TokenMixBar({ overview }: { overview: UsageOverviewModel }): React.JSX.Element {
  const segments = [
    {
      key: 'new-input',
      label: 'New input',
      value: overview.newInputTokens,
      className: 'bg-foreground'
    },
    {
      key: 'output',
      label: 'Output',
      value: overview.outputTokens,
      className: 'bg-muted-foreground'
    },
    {
      key: 'cache',
      label: 'Cache',
      value: overview.cacheTokens,
      className: 'bg-border'
    }
  ]
  // Why: Codex cached input is a subset of input. The overview model normalizes
  // that into new/cache buckets so the visual mix does not double-count it.
  const mixTotal = segments.reduce((sum, segment) => sum + segment.value, 0)

  return (
    <section className="rounded-lg border border-border/60 bg-card/40 p-4">
      <div className="mb-3 flex items-start justify-between gap-3">
        <div>
          <h4 className="text-sm font-semibold text-foreground">Token mix</h4>
          <p className="text-xs text-muted-foreground">
            Combined input, output, and cache tokens across enabled providers.
          </p>
        </div>
        {overview.reasoningTokens > 0 ? (
          <Badge variant="outline" className="shrink-0">
            {formatUsageTokens(overview.reasoningTokens)} reasoning
          </Badge>
        ) : null}
      </div>

      {mixTotal > 0 ? (
        <div
          className="flex h-3 overflow-hidden rounded-full border border-border/60 bg-muted"
          aria-label="Combined token mix"
        >
          {segments.map((segment) =>
            segment.value > 0 ? (
              <div
                key={segment.key}
                className={segment.className}
                style={{ width: `${(segment.value / mixTotal) * 100}%` }}
                aria-label={`${segment.label}: ${segment.value.toLocaleString()} tokens`}
              />
            ) : null
          )}
        </div>
      ) : (
        <div className="h-3 rounded-full border border-dashed border-border/60 bg-muted/40" />
      )}

      <div className="mt-3 grid gap-2 text-xs text-muted-foreground sm:grid-cols-3">
        {segments.map((segment) => (
          <div key={segment.key} className="flex min-w-0 items-center gap-2">
            <span className={`size-2 shrink-0 rounded-full ${segment.className}`} />
            <span className="min-w-0 truncate">
              {segment.label}: {formatUsageTokens(segment.value)}
            </span>
          </div>
        ))}
      </div>
    </section>
  )
}

function DailyIntensityGrid({
  days,
  bestDay
}: {
  days: UsageOverviewDailyPoint[]
  bestDay: UsageOverviewDailyPoint | null
}): React.JSX.Element {
  return (
    <section className="rounded-lg border border-border/60 bg-card/40 p-4">
      <div className="mb-3 flex items-start justify-between gap-3">
        <div>
          <h4 className="text-sm font-semibold text-foreground">Daily intensity</h4>
          <p className="text-xs text-muted-foreground">
            Recent combined Claude, Codex, and OpenCode token activity.
          </p>
        </div>
        {bestDay && bestDay.totalTokens > 0 ? (
          <Badge variant="outline" className="shrink-0">
            Best: {formatDayLabel(bestDay.day)}
          </Badge>
        ) : null}
      </div>

      <div
        className="grid grid-cols-[repeat(14,minmax(0,1fr))] gap-1 sm:grid-cols-[repeat(21,minmax(0,1fr))]"
        aria-label="Recent token activity heatmap"
      >
        {days.map((day) => (
          <div
            key={day.day}
            className={`aspect-square min-h-3 rounded-[2px] border ${INTENSITY_CLASS[day.intensity]}`}
            aria-label={`${day.day}: ${day.totalTokens.toLocaleString()} tokens`}
          />
        ))}
      </div>

      <div className="mt-3 flex items-center justify-between gap-3 text-xs text-muted-foreground">
        <span>{formatDayLabel(days[0]?.day ?? '')}</span>
        <span>Less</span>
        <div className="flex items-center gap-1" aria-hidden>
          {[0, 1, 2, 3, 4].map((intensity) => (
            <span
              key={intensity}
              className={`size-2 rounded-[2px] border ${INTENSITY_CLASS[intensity as UsageOverviewDailyPoint['intensity']]}`}
            />
          ))}
        </div>
        <span>More</span>
        <span>{formatDayLabel(days.at(-1)?.day ?? '')}</span>
      </div>
    </section>
  )
}

function ProviderRow({
  provider,
  totalTokens,
  onEnable
}: {
  provider: UsageProviderOverview
  totalTokens: number
  onEnable: () => void
}): React.JSX.Element {
  const share = totalTokens > 0 ? provider.totalTokens / totalTokens : 0
  // Why: comingSoon providers get a "Soon" badge and no enable action — consistent
  // with their dedicated tab which also shows a static "Soon" card.
  const status = provider.comingSoon
    ? 'Soon'
    : provider.enabled
      ? provider.isScanning
        ? 'Scanning'
        : 'Enabled'
      : 'Off'
  const statusVariant = provider.enabled ? 'secondary' : 'outline'

  return (
    <div className="rounded-lg border border-border/60 bg-card/40 p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <h5 className="truncate text-sm font-semibold text-foreground">{provider.label}</h5>
            <Badge variant={statusVariant}>{status}</Badge>
          </div>
          <p className="mt-1 truncate text-xs text-muted-foreground">
            {provider.topModel ?? 'No model yet'}
            {provider.topProject ? ` - ${provider.topProject}` : ''}
          </p>
        </div>
        {!provider.enabled && !provider.comingSoon ? (
          <Button variant="outline" size="xs" onClick={onEnable}>
            Enable
          </Button>
        ) : null}
      </div>

      <div className="mt-3 grid gap-2 text-xs text-muted-foreground sm:grid-cols-3">
        <span>{formatUsageTokens(provider.totalTokens)} tokens</span>
        <span>
          {provider.sessions.toLocaleString()} sessions - {provider.activityCount.toLocaleString()}{' '}
          {provider.activityLabel}
        </span>
        <span>{formatUsageCost(provider.estimatedCostUsd)}</span>
      </div>
      <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          className="h-full rounded-full bg-foreground/75"
          style={{ width: `${Math.max(share * 100, provider.totalTokens > 0 ? 2 : 0)}%` }}
        />
      </div>
      {provider.lastScanError ? (
        <p className="mt-2 flex items-center gap-1 text-xs text-destructive">
          <AlertCircle className="size-3" />
          {provider.lastScanError}
        </p>
      ) : null}
    </div>
  )
}

export function UsageOverviewPane(): React.JSX.Element {
  const claudeScanState = useAppStore((state) => state.claudeUsageScanState)
  const claudeSummary = useAppStore((state) => state.claudeUsageSummary)
  const claudeDaily = useAppStore((state) => state.claudeUsageDaily)
  const codexScanState = useAppStore((state) => state.codexUsageScanState)
  const codexSummary = useAppStore((state) => state.codexUsageSummary)
  const codexDaily = useAppStore((state) => state.codexUsageDaily)
  const openCodeScanState = useAppStore((state) => state.openCodeUsageScanState)
  const openCodeSummary = useAppStore((state) => state.openCodeUsageSummary)
  const openCodeDaily = useAppStore((state) => state.openCodeUsageDaily)
  const claudeUsageRange = useAppStore((state) => state.claudeUsageRange)
  const setClaudeUsageRange = useAppStore((state) => state.setClaudeUsageRange)
  const setCodexUsageRange = useAppStore((state) => state.setCodexUsageRange)
  const fetchClaudeUsage = useAppStore((state) => state.fetchClaudeUsage)
  const fetchCodexUsage = useAppStore((state) => state.fetchCodexUsage)
  const fetchOpenCodeUsage = useAppStore((state) => state.fetchOpenCodeUsage)
  const refreshClaudeUsage = useAppStore((state) => state.refreshClaudeUsage)
  const refreshCodexUsage = useAppStore((state) => state.refreshCodexUsage)
  const refreshOpenCodeUsage = useAppStore((state) => state.refreshOpenCodeUsage)
  const enableClaudeUsage = useAppStore((state) => state.enableClaudeUsage)
  const enableCodexUsage = useAppStore((state) => state.enableCodexUsage)
  const enableOpenCodeUsage = useAppStore((state) => state.enableOpenCodeUsage)
  const recordFeatureInteraction = useAppStore((state) => state.recordFeatureInteraction)

  const RANGE_OPTIONS = [
    { label: '7D', value: '7d' },
    { label: '30D', value: '30d' },
    { label: '90D', value: '90d' },
    { label: 'All', value: 'all' }
  ] as const

  const handleRangeChange = (range: (typeof RANGE_OPTIONS)[number]['value']): void => {
    void setClaudeUsageRange(range)
    // Why: the Overview range control drives both providers at once so the
    // combined stats (daily grid, token mix) stay coherent with one click.
    // OpenCode is excluded — it has no range API yet ("Soon").
    void setCodexUsageRange(range)
  }

  useEffect(() => {
    void fetchClaudeUsage()
    void fetchCodexUsage()
    void fetchOpenCodeUsage()
  }, [fetchClaudeUsage, fetchCodexUsage, fetchOpenCodeUsage])

  const overview = useMemo(
    () =>
      buildUsageOverview({
        claude: {
          scanState: claudeScanState,
          summary: claudeSummary,
          daily: claudeDaily
        },
        codex: {
          scanState: codexScanState,
          summary: codexSummary,
          daily: codexDaily
        },
        opencode: {
          scanState: openCodeScanState,
          summary: openCodeSummary,
          daily: openCodeDaily
        }
      }),
    [
      claudeDaily,
      claudeScanState,
      claudeSummary,
      codexDaily,
      codexScanState,
      codexSummary,
      openCodeDaily,
      openCodeScanState,
      openCodeSummary
    ]
  )
  const recentDays = useMemo(
    () => getRecentUsageDays(overview.daily, RECENT_DAY_COUNT),
    [overview.daily]
  )
  const isScanning = overview.providers.some((provider) => provider.isScanning)

  const handleRefresh = (): void => {
    void Promise.all([
      claudeScanState?.enabled ? refreshClaudeUsage() : Promise.resolve(),
      codexScanState?.enabled ? refreshCodexUsage() : Promise.resolve(),
      openCodeScanState?.enabled ? refreshOpenCodeUsage() : Promise.resolve()
    ])
  }

  return (
    <div className="space-y-4" data-testid="usage-overview-pane">
      <section className="rounded-lg border border-border/60 bg-card/30 p-4">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <h3 className="text-sm font-semibold text-foreground">Usage Overview</h3>
            <p className="mt-1 text-xs text-muted-foreground">
              {formatUpdatedAt(overview.lastUpdatedAt)}
              {overview.hasPartialCost ? ' - some model prices are unavailable' : ''}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <div
              className="flex items-center rounded-md border border-border/60 bg-muted/30 p-0.5 text-xs"
              role="group"
              aria-label="Usage date range"
            >
              {RANGE_OPTIONS.map((opt) => (
                <button
                  key={opt.value}
                  type="button"
                  className={`rounded px-2 py-0.5 font-medium transition-colors ${
                    claudeUsageRange === opt.value
                      ? 'bg-background text-foreground shadow-sm'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                  onClick={() => handleRangeChange(opt.value)}
                >
                  {opt.label}
                </button>
              ))}
            </div>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  onClick={handleRefresh}
                  disabled={!overview.hasAnyEnabledProvider || isScanning}
                  aria-label="Refresh usage overview"
                >
                  <RefreshCw className={`size-3.5 ${isScanning ? 'animate-spin' : ''}`} />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={6}>
                Refresh
              </TooltipContent>
            </Tooltip>
          </div>
        </div>

        {!overview.hasAnyEnabledProvider ? (
          <div className="mt-4 rounded-lg border border-dashed border-border/60 bg-card/30 px-4 py-5">
            <div className="max-w-xl space-y-3">
              <div>
                <h4 className="text-sm font-semibold text-foreground">Start tracking tokens</h4>
                <p className="mt-1 text-sm text-muted-foreground">
                  Enable a provider to scan local agent logs and build the combined token ledger.
                </p>
              </div>
              <div className="flex flex-wrap gap-2">
                <Button
                  size="sm"
                  onClick={() => {
                    recordFeatureInteraction('usage-tracking')
                    void enableClaudeUsage()
                  }}
                >
                  Enable Claude
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    recordFeatureInteraction('usage-tracking')
                    void enableCodexUsage()
                  }}
                >
                  Enable Codex
                </Button>
              </div>
            </div>
          </div>
        ) : (
          <>
            <div className="mt-4 grid gap-3 md:grid-cols-2 xl:grid-cols-4">
              <StatCard
                label="Total tokens"
                value={formatUsageTokens(overview.totalTokens)}
                icon={<Sparkles className="size-4" />}
              />
              <StatCard
                label="Est. cost"
                value={formatUsageCost(overview.estimatedCostUsd)}
                icon={<Coins className="size-4" />}
              />
              <StatCard
                label="Active days"
                value={overview.activeDays.toLocaleString()}
                icon={<CalendarDays className="size-4" />}
              />
              <StatCard
                label="Cache share"
                value={formatPercent(overview.cacheShare)}
                icon={<DatabaseZap className="size-4" />}
              />
            </div>

            {!overview.hasAnyData ? (
              <div className="mt-4 rounded-lg border border-dashed border-border/60 bg-card/30 px-4 py-5 text-sm text-muted-foreground">
                No local Claude, Codex, or OpenCode usage found yet. The overview will populate
                after the next agent session writes token logs.
              </div>
            ) : (
              <div className="mt-4 grid gap-4 xl:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]">
                <DailyIntensityGrid days={recentDays} bestDay={overview.bestDay} />
                <TokenMixBar overview={overview} />
              </div>
            )}
          </>
        )}
      </section>

      <section className="space-y-3">
        <div className="flex items-center justify-between gap-3">
          <div>
            <h4 className="text-sm font-semibold text-foreground">Providers</h4>
            <p className="text-xs text-muted-foreground">
              {overview.enabledProviderCount} enabled - {overview.dataProviderCount} with data
            </p>
          </div>
          <Badge variant="outline" className="gap-1">
            <Activity className="size-3" />
            {overview.sessions.toLocaleString()} sessions
          </Badge>
        </div>
        <div className="grid gap-3 xl:grid-cols-2">
          {overview.providers.map((provider) => (
            <ProviderRow
              key={provider.id}
              provider={provider}
              totalTokens={overview.totalTokens}
              onEnable={() => {
                recordFeatureInteraction('usage-tracking')
                if (provider.id === 'claude') {
                  void enableClaudeUsage()
                } else if (provider.id === 'codex') {
                  void enableCodexUsage()
                } else {
                  void enableOpenCodeUsage()
                }
              }}
            />
          ))}
        </div>
      </section>
    </div>
  )
}
