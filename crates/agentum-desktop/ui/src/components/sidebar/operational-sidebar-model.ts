import type { WorktreeStatus } from '@/lib/worktree-status'
import { isExplicitAgentStatusFresh } from '@/lib/agent-status'
import {
  AGENT_STATUS_STALE_AFTER_MS,
  type AgentStatusEntry
} from '@/shared/agent-status-types'
import type { Repo, Worktree } from '@/shared/types'
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
  /**
   * A completed agent turn that is newer than the user's last acknowledgement.
   * Kept separate from `status` because another pane may still be working while
   * this completion is waiting to be reviewed.
   */
  continuation?: OperationalContinuation
}

export type OperationalContinuation = {
  stateTimestamp: number
  agentLabel?: string
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
  // A done hook means the agent's turn stopped, not that the user has dealt
  // with the result. Unacknowledged completions are promoted to Needs You by
  // `continuation`; acknowledged completions belong with other quiet work.
  done: { section: 'settled', statusLabel: 'Settled' },
  // A mounted tab, browser surface, or live tmux session is only liveness. It
  // does not mean an agent is doing work, so keep it out of the action queue.
  active: { section: 'settled', statusLabel: 'Settled' },
  inactive: { section: 'settled', statusLabel: 'Settled' }
}

const CONTINUATION_META: Pick<OperationalWorkspaceMeta, 'section' | 'statusLabel'> = {
  section: 'needs-you',
  statusLabel: 'Ready to continue'
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

/**
 * Return the newest current `done` state the user has not viewed yet.
 *
 * This intentionally does not apply the live-status freshness TTL. A done
 * transition is a durable event (the Activity badge follows the same
 * acknowledgement contract), not a claim that an agent is still executing.
 * It remains actionable until the user visits that exact pane, including when
 * the entry has moved to retained state or the app has restarted.
 */
export function selectOperationalContinuation(
  entries: readonly Pick<
    AgentStatusEntry,
    'agentType' | 'paneKey' | 'state' | 'stateStartedAt' | 'updatedAt'
  >[],
  acknowledgedAgentsByPaneKey: Readonly<Record<string, number>>
): OperationalContinuation | undefined {
  let winner:
    | Pick<
        AgentStatusEntry,
        'agentType' | 'paneKey' | 'state' | 'stateStartedAt' | 'updatedAt'
      >
    | undefined

  for (const entry of entries) {
    const stateTimestamp = finiteTimestamp(entry.stateStartedAt)
    const acknowledgedAt = finiteTimestamp(acknowledgedAgentsByPaneKey[entry.paneKey]) ?? 0
    if (
      entry.state !== 'done' ||
      stateTimestamp === undefined ||
      acknowledgedAt >= stateTimestamp
    ) {
      continue
    }
    if (
      !winner ||
      entry.stateStartedAt > winner.stateStartedAt ||
      (entry.stateStartedAt === winner.stateStartedAt && entry.updatedAt > winner.updatedAt)
    ) {
      winner = entry
    }
  }

  if (!winner) return undefined
  return {
    stateTimestamp: winner.stateStartedAt,
    agentLabel: winner.agentType?.trim() || undefined
  }
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
    // Explicit input/permission requests remain the most urgent signal. An
    // unseen completion otherwise outranks working liveness: split panes can
    // have one agent waiting for review while another continues executing.
    const continuation = fact.status === 'permission' ? undefined : fact.continuation
    const agentLabel = continuation?.agentLabel ?? fact.agentLabel
    if (
      normalizedQuery &&
      !normalizedSearchText([
        worktree.displayName,
        worktree.branch,
        repo?.displayName,
        agentLabel
      ]).includes(normalizedQuery)
    ) {
      continue
    }

    const statusMeta = continuation ? CONTINUATION_META : STATUS_META[fact.status]
    const timestamp =
      continuation !== undefined
        ? finiteTimestamp(continuation.stateTimestamp)
        : fact.status === 'inactive'
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
      agentLabel: agentLabel?.trim() || undefined,
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
