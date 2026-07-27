import type { RetainedAgentEntry } from '@/store/slices/agent-status'
import type { AppState } from '@/store/types'
import type {
  AgentStatusEntry,
  MigrationUnsupportedPtyEntry
} from '@/shared/agent-status-types'
import { parsePaneKey } from '@/shared/stable-pane-id'
import type { TerminalLayoutSnapshot } from '@/shared/types'

const EMPTY_LIVE_ENTRIES: AgentStatusEntry[] = []
const EMPTY_MIGRATION_UNSUPPORTED_ENTRIES: MigrationUnsupportedPtyEntry[] = []
const EMPTY_RETAINED: RetainedAgentEntry[] = []

type WorktreeAgentRowsState = Pick<
  AppState,
  | 'agentStatusByPaneKey'
  | 'migrationUnsupportedByPtyId'
  | 'retainedAgentsByPaneKey'
  | 'tabsByWorktree'
  | 'serverAgentDoneByPaneKey'
>

// Stable empty fallbacks so a partial state (e.g. a minimal store mock under
// the sidebar ctx% chip) doesn't crash the selector, while keeping a constant
// identity so the memo caches below still hit. Real store state never omits these.
const EMPTY_AGENT_STATUS_BY_PANE_KEY: WorktreeAgentRowsState['agentStatusByPaneKey'] = {}
const EMPTY_TABS_BY_WORKTREE: WorktreeAgentRowsState['tabsByWorktree'] =
  {} as WorktreeAgentRowsState['tabsByWorktree']
const EMPTY_SERVER_AGENT_DONE_BY_PANE_KEY: WorktreeAgentRowsState['serverAgentDoneByPaneKey'] = {}
const EMPTY_SERVER_AGENT_DONE: Record<string, number> = {}

type TabWorktreeIndexCache = {
  tabsByWorktree: WorktreeAgentRowsState['tabsByWorktree']
  tabIdToWorktreeId: Map<string, string>
}

type LiveEntriesByWorktreeCache = {
  tabsByWorktree: WorktreeAgentRowsState['tabsByWorktree']
  agentStatusByPaneKey: WorktreeAgentRowsState['agentStatusByPaneKey']
  entriesByWorktree: Map<string, AgentStatusEntry[]>
}

type MigrationUnsupportedByWorktreeCache = {
  tabsByWorktree: WorktreeAgentRowsState['tabsByWorktree']
  migrationUnsupportedByPtyId: WorktreeAgentRowsState['migrationUnsupportedByPtyId']
  entriesByWorktree: Map<string, MigrationUnsupportedPtyEntry[]>
}

type RetainedEntriesByWorktreeCache = {
  retainedAgentsByPaneKey: WorktreeAgentRowsState['retainedAgentsByPaneKey']
  entriesByWorktree: Map<string, RetainedAgentEntry[]>
}

let tabWorktreeIndexCache: TabWorktreeIndexCache | null = null
let liveEntriesByWorktreeCache: LiveEntriesByWorktreeCache | null = null
let migrationUnsupportedByWorktreeCache: MigrationUnsupportedByWorktreeCache | null = null
let retainedEntriesByWorktreeCache: RetainedEntriesByWorktreeCache | null = null

function reuseArrayIfEqual<T>(previous: T[] | undefined, next: T[]): T[] {
  if (!previous || previous.length !== next.length) {
    return next
  }
  for (let i = 0; i < next.length; i += 1) {
    if (previous[i] !== next[i]) {
      return next
    }
  }
  return previous
}

function getTabIdToWorktreeId(
  tabsByWorktree: WorktreeAgentRowsState['tabsByWorktree']
): Map<string, string> {
  if (tabWorktreeIndexCache?.tabsByWorktree === tabsByWorktree) {
    return tabWorktreeIndexCache.tabIdToWorktreeId
  }
  const tabIdToWorktreeId = new Map<string, string>()
  for (const [worktreeId, tabs] of Object.entries(tabsByWorktree)) {
    for (const tab of tabs) {
      tabIdToWorktreeId.set(tab.id, worktreeId)
    }
  }
  tabWorktreeIndexCache = { tabsByWorktree, tabIdToWorktreeId }
  return tabIdToWorktreeId
}

function getLiveEntriesByWorktree(state: WorktreeAgentRowsState): Map<string, AgentStatusEntry[]> {
  // Default to STABLE empty constants when a (partial) state omits these — keeps
  // identity stable so the cache below still hits, while tolerating callers
  // (e.g. the sidebar ctx% chip under minimal store mocks) whose state lacks
  // the agent-status fields. Real store state always provides both.
  const tabsByWorktree = state.tabsByWorktree ?? EMPTY_TABS_BY_WORKTREE
  const agentStatusByPaneKey = state.agentStatusByPaneKey ?? EMPTY_AGENT_STATUS_BY_PANE_KEY
  if (
    liveEntriesByWorktreeCache?.tabsByWorktree === tabsByWorktree &&
    liveEntriesByWorktreeCache.agentStatusByPaneKey === agentStatusByPaneKey
  ) {
    return liveEntriesByWorktreeCache.entriesByWorktree
  }

  const tabIdToWorktreeId = getTabIdToWorktreeId(tabsByWorktree)
  const previous = liveEntriesByWorktreeCache?.entriesByWorktree
  const entriesByWorktree = new Map<string, AgentStatusEntry[]>()
  for (const [paneKey, entry] of Object.entries(agentStatusByPaneKey)) {
    const parsed = parsePaneKey(paneKey)
    if (!parsed) {
      continue
    }
    const worktreeId = tabIdToWorktreeId.get(parsed.tabId)
    if (!worktreeId) {
      continue
    }
    const bucket = entriesByWorktree.get(worktreeId)
    if (bucket) {
      bucket.push(entry)
    } else {
      entriesByWorktree.set(worktreeId, [entry])
    }
  }
  for (const [worktreeId, entries] of entriesByWorktree) {
    entriesByWorktree.set(worktreeId, reuseArrayIfEqual(previous?.get(worktreeId), entries))
  }
  liveEntriesByWorktreeCache = {
    tabsByWorktree,
    agentStatusByPaneKey,
    entriesByWorktree
  }
  return entriesByWorktree
}

function getMigrationUnsupportedByWorktree(
  state: WorktreeAgentRowsState
): Map<string, MigrationUnsupportedPtyEntry[]> {
  if (
    migrationUnsupportedByWorktreeCache?.tabsByWorktree === state.tabsByWorktree &&
    migrationUnsupportedByWorktreeCache.migrationUnsupportedByPtyId ===
      state.migrationUnsupportedByPtyId
  ) {
    return migrationUnsupportedByWorktreeCache.entriesByWorktree
  }

  const tabIdToWorktreeId = getTabIdToWorktreeId(state.tabsByWorktree)
  const previous = migrationUnsupportedByWorktreeCache?.entriesByWorktree
  const entriesByWorktree = new Map<string, MigrationUnsupportedPtyEntry[]>()
  for (const unsupported of Object.values(state.migrationUnsupportedByPtyId)) {
    if (!unsupported.paneKey) {
      continue
    }
    const parsed = parsePaneKey(unsupported.paneKey)
    const worktreeId = parsed ? tabIdToWorktreeId.get(parsed.tabId) : undefined
    if (!worktreeId) {
      continue
    }
    const bucket = entriesByWorktree.get(worktreeId)
    if (bucket) {
      bucket.push(unsupported)
    } else {
      entriesByWorktree.set(worktreeId, [unsupported])
    }
  }
  for (const [worktreeId, entries] of entriesByWorktree) {
    entriesByWorktree.set(worktreeId, reuseArrayIfEqual(previous?.get(worktreeId), entries))
  }
  migrationUnsupportedByWorktreeCache = {
    tabsByWorktree: state.tabsByWorktree,
    migrationUnsupportedByPtyId: state.migrationUnsupportedByPtyId,
    entriesByWorktree
  }
  return entriesByWorktree
}

function getRetainedEntriesByWorktree(
  state: WorktreeAgentRowsState
): Map<string, RetainedAgentEntry[]> {
  if (retainedEntriesByWorktreeCache?.retainedAgentsByPaneKey === state.retainedAgentsByPaneKey) {
    return retainedEntriesByWorktreeCache.entriesByWorktree
  }

  const previous = retainedEntriesByWorktreeCache?.entriesByWorktree
  const entriesByWorktree = new Map<string, RetainedAgentEntry[]>()
  for (const retained of Object.values(state.retainedAgentsByPaneKey)) {
    const bucket = entriesByWorktree.get(retained.worktreeId)
    if (bucket) {
      bucket.push(retained)
    } else {
      entriesByWorktree.set(retained.worktreeId, [retained])
    }
  }
  for (const [worktreeId, entries] of entriesByWorktree) {
    entriesByWorktree.set(worktreeId, reuseArrayIfEqual(previous?.get(worktreeId), entries))
  }
  retainedEntriesByWorktreeCache = {
    retainedAgentsByPaneKey: state.retainedAgentsByPaneKey,
    entriesByWorktree
  }
  return entriesByWorktree
}

// Why: serverAgentDoneByPaneKey (title-derived "done" markers for hook-less
// server sessions) was the ONE input useWorktreeAgentRows read via an inline
// Object.entries filter — O(done-agents) per card on EVERY store commit, i.e.
// O(cards × done-agents) of work per commit under many agents. Index it per
// worktree like every sibling input so each card does an O(1) lookup and the
// bucketing rebuilds only when the done-map or tab map identity changes.
let serverAgentDoneByWorktreeCache: {
  serverAgentDoneByPaneKey: WorktreeAgentRowsState['serverAgentDoneByPaneKey']
  tabsByWorktree: WorktreeAgentRowsState['tabsByWorktree']
  byWorktree: Map<string, Record<string, number>>
} | null = null

function getServerAgentDoneByWorktree(
  state: WorktreeAgentRowsState
): Map<string, Record<string, number>> {
  const serverAgentDoneByPaneKey =
    state.serverAgentDoneByPaneKey ?? EMPTY_SERVER_AGENT_DONE_BY_PANE_KEY
  const tabsByWorktree = state.tabsByWorktree ?? EMPTY_TABS_BY_WORKTREE
  if (
    serverAgentDoneByWorktreeCache?.serverAgentDoneByPaneKey === serverAgentDoneByPaneKey &&
    serverAgentDoneByWorktreeCache.tabsByWorktree === tabsByWorktree
  ) {
    return serverAgentDoneByWorktreeCache.byWorktree
  }
  const tabIdToWorktreeId = getTabIdToWorktreeId(tabsByWorktree)
  const byWorktree = new Map<string, Record<string, number>>()
  for (const [paneKey, finishedAt] of Object.entries(serverAgentDoneByPaneKey)) {
    const parsed = parsePaneKey(paneKey)
    if (!parsed) {
      continue
    }
    const worktreeId = tabIdToWorktreeId.get(parsed.tabId)
    if (!worktreeId) {
      continue
    }
    const bucket = byWorktree.get(worktreeId)
    if (bucket) {
      bucket[paneKey] = finishedAt
    } else {
      byWorktree.set(worktreeId, { [paneKey]: finishedAt })
    }
  }
  serverAgentDoneByWorktreeCache = { serverAgentDoneByPaneKey, tabsByWorktree, byWorktree }
  return byWorktree
}

export function selectLiveAgentStatusEntriesForWorktree(
  state: WorktreeAgentRowsState,
  worktreeId: string
): AgentStatusEntry[] {
  return getLiveEntriesByWorktree(state).get(worktreeId) ?? EMPTY_LIVE_ENTRIES
}

export function selectServerAgentDoneForWorktree(
  state: WorktreeAgentRowsState,
  worktreeId: string
): Record<string, number> {
  return getServerAgentDoneByWorktree(state).get(worktreeId) ?? EMPTY_SERVER_AGENT_DONE
}

export function selectMigrationUnsupportedEntriesForWorktree(
  state: WorktreeAgentRowsState,
  worktreeId: string
): MigrationUnsupportedPtyEntry[] {
  return (
    getMigrationUnsupportedByWorktree(state).get(worktreeId) ?? EMPTY_MIGRATION_UNSUPPORTED_ENTRIES
  )
}

export function selectRetainedAgentEntriesForWorktree(
  state: WorktreeAgentRowsState,
  worktreeId: string
): RetainedAgentEntry[] {
  return getRetainedEntriesByWorktree(state).get(worktreeId) ?? EMPTY_RETAINED
}

export function selectTerminalLayoutsForWorktree(
  state: Pick<AppState, 'tabsByWorktree' | 'terminalLayoutsByTabId'>,
  worktreeId: string
): Record<string, TerminalLayoutSnapshot | undefined> {
  const out: Record<string, TerminalLayoutSnapshot | undefined> = {}
  for (const tab of state.tabsByWorktree[worktreeId] ?? []) {
    out[tab.id] = state.terminalLayoutsByTabId[tab.id]
  }
  return out
}
