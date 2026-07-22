import type { WorktreeStatus } from '@/lib/worktree-status'
import { isExplicitAgentStatusFresh } from '@/lib/agent-status'
import {
  AGENT_STATUS_STALE_AFTER_MS,
  type AgentStatusEntry
} from '../../shared/agent-status-types'
import type { Repo, Worktree } from '../../../../shared/types'
import type {
  OperationalSection,
  OperationalWorkspaceMeta,
  Row
} from './worktree-list-groups'

export type OperationalWorkspaceFact = {
  status: WorktreeStatus
  agentLabel?: string
  /** Timestamp for the winning status signal, in epoch milliseconds. */
  stateTimestamp?: number
}

export type BuildOperationalSidebarRowsArgs = {
  worktrees: readonly Worktree[]
  repoMap: ReadonlyMap<string, Repo>
  factsByWorktreeId: ReadonlyMap<string, OperationalWorkspaceFact>
  query?: string
  settledExpanded?: boolean
  settledLimit?: number
  now?: number
}

const SECTION_META: ReadonlyArray<{
  section: OperationalSection
  label: string
  tone: string
}> = [
  { section: 'needs-you', label: 'Needs You', tone: 'text-destructive' },
  { section: 'active', label: 'Active', tone: 'text-foreground' },
  { section: 'settled', label: 'Settled', tone: 'text-muted-foreground' }
]

const STATUS_META: Record<
  WorktreeStatus,
  Pick<OperationalWorkspaceMeta, 'section' | 'statusLabel'>
> = {
  permission: { section: 'needs-you', statusLabel: 'Needs input' },
  working: { section: 'active', statusLabel: 'Working' },
  done: { section: 'active', statusLabel: 'Ready' },
  active: { section: 'active', statusLabel: 'Active' },
  inactive: { section: 'settled', statusLabel: 'Settled' }
}

type OperationalEntry = {
  worktree: Worktree
  repo: Repo | undefined
  meta: OperationalWorkspaceMeta
}

function finiteTimestamp(value: number | undefined): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined
}

/**
 * Select the state start belonging to the same explicit pane signal as the
 * aggregate status. Watchdog/title/server fallbacks have no provable pane
 * timestamp, so callers intentionally omit the age for those signals.
 */
export function selectOperationalStatusTimestamp(
  status: WorktreeStatus,
  entries: readonly Pick<AgentStatusEntry, 'state' | 'stateStartedAt' | 'updatedAt'>[],
  now: number = Date.now()
): number | undefined {
  const matchesStatus = (state: AgentStatusEntry['state']): boolean => {
    if (status === 'permission') return state === 'blocked' || state === 'waiting'
    if (status === 'working') return state === 'working'
    if (status === 'done') return state === 'done'
    return false
  }

  let winner: (typeof entries)[number] | undefined
  for (const entry of entries) {
    if (
      isExplicitAgentStatusFresh(entry, now, AGENT_STATUS_STALE_AFTER_MS) &&
      matchesStatus(entry.state) &&
      (!winner || entry.updatedAt > winner.updatedAt)
    ) {
      winner = entry
    }
  }
  return finiteTimestamp(winner?.stateStartedAt)
}

export function formatOperationalShortAge(timestamp: number | undefined, now: number): string | undefined {
  const validTimestamp = finiteTimestamp(timestamp)
  if (validTimestamp === undefined || !Number.isFinite(now)) {
    return undefined
  }
  const seconds = Math.max(0, Math.floor((now - validTimestamp) / 1000))
  if (seconds < 60) return 'now'
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h`
  const days = Math.floor(hours / 24)
  if (days < 30) return `${days}d`
  const months = Math.floor(days / 30)
  if (months < 12) return `${months}mo`
  return `${Math.floor(months / 12)}y`
}

function normalizedSearchText(parts: Array<string | undefined>): string {
  return parts.filter(Boolean).join('\n').normalize('NFKC').toLocaleLowerCase()
}

function entryTimestamp(entry: OperationalEntry): number {
  return (
    finiteTimestamp(entry.meta.stateTimestamp) ??
    finiteTimestamp(entry.worktree.lastActivityAt) ??
    0
  )
}

function compareOperationalEntries(a: OperationalEntry, b: OperationalEntry): number {
  if (a.worktree.isPinned !== b.worktree.isPinned) {
    return a.worktree.isPinned ? -1 : 1
  }
  const byActivity = entryTimestamp(b) - entryTimestamp(a)
  if (byActivity !== 0) return byActivity
  return a.worktree.displayName.localeCompare(b.worktree.displayName, undefined, {
    sensitivity: 'base'
  })
}

function compareSettledEntries(a: OperationalEntry, b: OperationalEntry): number {
  const byActivity = entryTimestamp(b) - entryTimestamp(a)
  if (byActivity !== 0) return byActivity
  return a.worktree.displayName.localeCompare(b.worktree.displayName, undefined, {
    sensitivity: 'base'
  })
}

function itemRow(entry: OperationalEntry): Extract<Row, { type: 'item' }> {
  return {
    type: 'item',
    worktree: entry.worktree,
    repo: entry.repo,
    depth: 0,
    lineageTrail: [],
    isLastLineageChild: false,
    lineageChildCount: 0,
    presentation: entry.meta.presentation,
    operationalMeta: entry.meta
  }
}

/** Build the exact flat row sequence consumed by the existing sidebar virtualizer. */
export function buildOperationalSidebarRows({
  worktrees,
  repoMap,
  factsByWorktreeId,
  query = '',
  settledExpanded = false,
  settledLimit = 3,
  now = Date.now()
}: BuildOperationalSidebarRowsArgs): Row[] {
  const buckets: Record<OperationalSection, OperationalEntry[]> = {
    'needs-you': [],
    active: [],
    settled: []
  }
  const normalizedQuery = query.trim().normalize('NFKC').toLocaleLowerCase()

  for (const worktree of worktrees) {
    const fact = factsByWorktreeId.get(worktree.id) ?? { status: 'inactive' as const }
    const repo = repoMap.get(worktree.repoId)
    if (
      normalizedQuery &&
      !normalizedSearchText([
        worktree.displayName,
        worktree.branch,
        repo?.displayName,
        fact.agentLabel
      ]).includes(normalizedQuery)
    ) {
      continue
    }

    const statusMeta = STATUS_META[fact.status]
    const timestamp =
      fact.status === 'inactive'
        ? finiteTimestamp(worktree.lastActivityAt)
        : finiteTimestamp(fact.stateTimestamp)
    const relativeAge = formatOperationalShortAge(timestamp, now)
    const meta: OperationalWorkspaceMeta = {
      presentation:
        statusMeta.section === 'settled' ? 'operational-settled' : 'operational-rich',
      section: statusMeta.section,
      status: fact.status,
      statusLabel: statusMeta.statusLabel,
      projectName: repo?.displayName,
      agentLabel: fact.agentLabel?.trim() || undefined,
      stateTimestamp: timestamp,
      ageLabel: relativeAge,
      relativeAge
    }
    buckets[meta.section].push({ worktree, repo, meta })
  }

  buckets['needs-you'].sort(compareOperationalEntries)
  buckets.active.sort(compareOperationalEntries)
  buckets.settled.sort(compareSettledEntries)

  const rows: Row[] = []
  for (const { section, label, tone } of SECTION_META) {
    const entries = buckets[section]
    rows.push({
      type: 'header',
      key: `operational:${section}`,
      label,
      count: entries.length,
      tone
    })
    const visible =
      section === 'settled' && !settledExpanded
        ? entries.slice(0, Math.max(0, settledLimit))
        : entries
    rows.push(...visible.map(itemRow))
    if (section === 'settled' && entries.length > settledLimit) {
      rows.push({
        type: 'operational-settled-disclosure',
        key: 'operational:settled:disclosure',
        remainingCount: settledExpanded ? entries.length - settledLimit : entries.length - visible.length,
        expanded: settledExpanded
      })
    }
  }
  return rows
}
