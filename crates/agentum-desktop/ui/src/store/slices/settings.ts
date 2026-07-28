import { api } from '@/tauri'
/* eslint-disable max-lines */
import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import type { GlobalSettings } from '@/shared/types'
import { toast } from 'sonner'
import { callRuntimeRpc, clearRuntimeCompatibilityCache } from '@/runtime/runtime-rpc-client'
import {
  getRemoteRuntimePtyEnvironmentId,
  getRemoteRuntimeTerminalHandle
} from '@/runtime/runtime-terminal-stream'
import { getDefaultSettings } from '@/shared/constants'
import { normalizeTerminalQuickCommands } from '@/shared/terminal-quick-commands'
import { normalizeTaskProviderSettings } from '@/shared/task-providers'
import { normalizeOpenInApplications } from '@/shared/open-in-applications'
import { createSettingsSearchState, type SettingsSearchState } from './settings-search-state'
import { normalizeDisabledTuiAgents } from '@/shared/tui-agent-selection'

export type SettingsSlice = SettingsSearchState & {
  settings: GlobalSettings | null
  fetchSettings: () => Promise<void>
  updateSettings: (updates: Partial<GlobalSettings>) => Promise<void>
  switchRuntimeEnvironment: (environmentId: string | null) => Promise<boolean>
}

function normalizeRuntimeEnvironmentId(value: string | null | undefined): string | null {
  const trimmed = value?.trim()
  return trimmed ? trimmed : null
}

function createOpenInApplicationId(): string {
  return (
    globalThis.crypto?.randomUUID?.() ??
    `open-in-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
  )
}

function runtimeScopedStateReset(): Partial<AppState> {
  return {
    repos: [],
    projectGroups: [],
    activeRepoId: null,
    sparsePresetsByRepo: {},
    sparsePresetsLoadingByRepo: {},
    sparsePresetsLoadStatusByRepo: {},
    sparsePresetsErrorByRepo: {},
    worktreesByRepo: {},
    detectedWorktreesByRepo: {},
    worktreeLineageById: {},
    activeWorktreeId: null,
    deleteStateByWorktreeId: {},
    baseStatusByWorktreeId: {},
    remoteBranchConflictByWorktreeId: {},
    sortEpoch: 0,
    everActivatedWorktreeIds: new Set<string>(),
    lastVisitedAtByWorktreeId: {},
    hasHydratedWorktreePurge: false,
    unifiedTabsByWorktree: {},
    groupsByWorktree: {},
    activeGroupIdByWorktree: {},
    layoutByWorktree: {},
    tabsByWorktree: {},
    activeTabId: null,
    activeTabIdByWorktree: {},
    ptyIdsByTabId: {},
    runtimePaneTitlesByTabId: {},
    unreadTerminalTabs: {},
    suppressedPtyExitIds: {},
    pendingCodexPaneRestartIds: {},
    codexRestartNoticeByPtyId: {},
    expandedPaneByTabId: {},
    canExpandPaneByTabId: {},
    terminalLayoutsByTabId: {},
    pendingStartupByTabId: {},
    pendingSetupSplitByTabId: {},
    pendingIssueCommandSplitByTabId: {},
    tabBarOrderByWorktree: {},
    pendingReconnectWorktreeIds: [],
    pendingReconnectTabByWorktree: {},
    pendingReconnectPtyIdByTabId: {},
    lastKnownRelayPtyIdByTabId: {},
    pendingSnapshotByPtyId: {},
    pendingColdRestoreByPtyId: {},
    deferredSshReconnectTargets: [],
    deferredSshSessionIdsByTabId: {},
    cacheTimerByKey: {},
    recentQuickCommandIdByGroup: {},
    showDotfilesByWorktree: {},
    expandedDirs: {},
    pendingExplorerReveal: null,
    openFiles: [],
    editorDrafts: {},
    markdownViewMode: {},
    editorViewMode: {},
    editorCursorLine: {},
    gitIgnoredPathsByWorktree: {},
    activeFileId: null,
    activeFileIdByWorktree: {},
    activeTabTypeByWorktree: {},
    activeTabType: 'terminal',
    recentlyClosedEditorTabsByWorktree: {},
    browserTabsByWorktree: {},
    browserPagesByWorkspace: {},
    browserAnnotationsByPageId: {},
    remoteBrowserPageHandlesByPageId: {},
    activeBrowserTabId: null,
    activeBrowserTabIdByWorktree: {},
    recentlyClosedBrowserTabsByWorktree: {},
    recentlyClosedBrowserPagesByWorkspace: {},
    pendingAddressBarFocusByTabId: {},
    pendingAddressBarFocusByPageId: {},
    browserSessionProfiles: [],
    browserSessionImportState: null,
    defaultBrowserSessionProfileId: null,
    detectedBrowsers: [],
    detectedBrowsersLoaded: false,
    prCache: {},
    issueCache: {},
    checksCache: {},
    commentsCache: {},
    workItemsCache: {},
    workItemsInvalidationNonce: 0,
    projectViewCache: {},
    linearStatus: { connected: false, viewer: null },
    linearStatusChecked: false,
    linearIssueCache: {},
    linearSearchCache: {},
    linearTeamCache: {},
    linearProjectCache: {},
    linearProjectDetailCache: {},
    linearProjectIssueCache: {},
    linearCustomViewCache: {},
    linearCustomViewDetailCache: {},
    linearCustomViewIssueCache: {},
    linearCustomViewProjectCache: {}
  }
}

function hasUnsavedEditorState(state: AppState): boolean {
  return state.openFiles.some((file) => file.isDirty || state.editorDrafts[file.id] !== undefined)
}

function isPairedWebClient(): boolean {
  return Boolean((globalThis as { __AGENTUM_WEB_CLIENT__?: boolean }).__AGENTUM_WEB_CLIENT__)
}

async function closeRemoteBrowserPagesBeforeRuntimeSwitch(state: AppState): Promise<void> {
  const worktreeIdByPageId = new Map<string, string>()
  for (const pages of Object.values(state.browserPagesByWorkspace)) {
    for (const page of pages) {
      worktreeIdByPageId.set(page.id, page.worktreeId)
    }
  }
  await Promise.allSettled(
    Object.entries(state.remoteBrowserPageHandlesByPageId).map(([pageId, handle]) => {
      const worktreeId = worktreeIdByPageId.get(pageId)
      if (!worktreeId) {
        return Promise.resolve()
      }
      return callRuntimeRpc(
        { kind: 'environment', environmentId: handle.environmentId },
        'browser.tabClose',
        { worktree: `id:${worktreeId}`, page: handle.remotePageId },
        { timeoutMs: 15_000 }
      )
    })
  )
}

function collectRemoteTerminalHandlesForRuntimeSwitch(
  state: AppState,
  fallbackEnvironmentId: string | null
): Map<string, Set<string>> {
  const handlesByEnvironmentId = new Map<string, Set<string>>()
  const collect = (ptyId: string | null | undefined): void => {
    if (!ptyId) {
      return
    }
    const handle = getRemoteRuntimeTerminalHandle(ptyId)
    if (!handle) {
      return
    }
    const environmentId = getRemoteRuntimePtyEnvironmentId(ptyId) ?? fallbackEnvironmentId
    if (!environmentId) {
      return
    }
    const handles = handlesByEnvironmentId.get(environmentId) ?? new Set<string>()
    handles.add(handle)
    handlesByEnvironmentId.set(environmentId, handles)
  }

  for (const ptyIds of Object.values(state.ptyIdsByTabId)) {
    for (const ptyId of ptyIds) {
      collect(ptyId)
    }
  }
  for (const tabs of Object.values(state.tabsByWorktree)) {
    for (const tab of tabs) {
      collect(tab.ptyId)
    }
  }
  for (const layout of Object.values(state.terminalLayoutsByTabId)) {
    for (const ptyId of Object.values(layout.ptyIdsByLeafId ?? {})) {
      collect(ptyId)
    }
  }
  return handlesByEnvironmentId
}

async function closeRemoteTerminalsBeforeRuntimeSwitch(
  state: AppState,
  fallbackEnvironmentId: string | null
): Promise<void> {
  const handlesByEnvironmentId = collectRemoteTerminalHandlesForRuntimeSwitch(
    state,
    fallbackEnvironmentId
  )
  await Promise.allSettled(
    Array.from(handlesByEnvironmentId.entries()).flatMap(([environmentId, handles]) =>
      Array.from(handles).map((terminal) =>
        callRuntimeRpc(
          { kind: 'environment', environmentId },
          'terminal.close',
          { terminal },
          { timeoutMs: 15_000 }
        )
      )
    )
  )
}

async function verifyRuntimeEnvironmentReachable(environmentId: string | null): Promise<void> {
  if (!environmentId) {
    return
  }
  await callRuntimeRpc({ kind: 'environment', environmentId }, 'repo.list', undefined, {
    timeoutMs: 15_000
  })
}

export const createSettingsSlice: StateCreator<AppState, [], [], SettingsSlice> = (set, get) => ({
  settings: null,
  ...createSettingsSearchState((state) => set(state)),

  fetchSettings: async () => {
    try {
      // settings.get() returns only the keys that were explicitly stored; merge
      // over defaults so `settings` is always a complete GlobalSettings object
      // (avoids `undefined` reads like buildFontFamily(settings.terminalFontFamily)).
      const stored = (await api.settings.get()) as Partial<GlobalSettings> | null
      set({ settings: { ...getDefaultSettings('~'), ...(stored ?? {}) } })
    } catch (err) {
      console.error('Failed to fetch settings:', err)
    }
  },

  updateSettings: async (updates) => {
    try {
      const sanitizedUpdates = { ...updates }
      if ('terminalQuickCommands' in updates) {
        sanitizedUpdates.terminalQuickCommands = normalizeTerminalQuickCommands(
          updates.terminalQuickCommands
        )
      }
      if ('visibleTaskProviders' in updates || 'defaultTaskSource' in updates) {
        const taskProviderSettings = normalizeTaskProviderSettings({
          visibleTaskProviders:
            'visibleTaskProviders' in updates
              ? updates.visibleTaskProviders
              : get().settings?.visibleTaskProviders,
          defaultTaskSource:
            'defaultTaskSource' in updates
              ? updates.defaultTaskSource
              : get().settings?.defaultTaskSource
        })
        sanitizedUpdates.defaultTaskSource = taskProviderSettings.defaultTaskSource
        sanitizedUpdates.visibleTaskProviders = taskProviderSettings.visibleTaskProviders
      }
      if ('openInApplications' in updates) {
        sanitizedUpdates.openInApplications = normalizeOpenInApplications(
          updates.openInApplications,
          {
            createId: createOpenInApplicationId
          }
        )
      }
      if ('disabledTuiAgents' in updates) {
        sanitizedUpdates.disabledTuiAgents = normalizeDisabledTuiAgents(updates.disabledTuiAgents)
      }
      const nextSettings = await api.settings.set(sanitizedUpdates)
      // Merge over the (complete) in-memory settings so the result stays a full
      // GlobalSettings even though set() returns only the stored keys.
      set((s) => ({
        settings: {
          ...(s.settings ?? getDefaultSettings('~')),
          ...((nextSettings as Partial<GlobalSettings> | undefined) ?? {})
        }
      }))
    } catch (err) {
      console.error('Failed to update settings:', err)
    }
  },

  switchRuntimeEnvironment: async (environmentId) => {
    const nextId = normalizeRuntimeEnvironmentId(environmentId)
    const previousId = normalizeRuntimeEnvironmentId(get().settings?.activeRuntimeEnvironmentId)
    if (previousId === nextId) {
      return true
    }
    if (hasUnsavedEditorState(get())) {
      toast.error('Save or close unsaved editor tabs before switching servers.')
      return false
    }
    try {
      clearRuntimeCompatibilityCache(nextId)
      await verifyRuntimeEnvironmentReachable(nextId)
      if (!isPairedWebClient()) {
        // Why: desktop-created remote resources live on their owning server.
        // Paired web clients only mirror host-owned tabs/PTYs, so switching
        // pairings must detach local state without killing the host session.
        await closeRemoteTerminalsBeforeRuntimeSwitch(get(), previousId)
        await closeRemoteBrowserPagesBeforeRuntimeSwitch(get())
      }
      const nextSettings = await api.settings.set({
        activeRuntimeEnvironmentId: nextId
      })
      set((s) => ({
        ...runtimeScopedStateReset(),
        settings: {
          ...(s.settings ?? getDefaultSettings('~')),
          ...((nextSettings as Partial<GlobalSettings> | undefined) ?? {
            activeRuntimeEnvironmentId: nextId
          })
        }
      }))
      // Why: server-owned state is cleared before refetch so old worktree,
      // terminal, browser, and issue IDs cannot be used against the new server
      // while the new environment is loading.
      await get().fetchRepos()
      await get().fetchProjectGroups()
      await get().fetchAllWorktrees()
      await get().fetchWorktreeLineage()
      await get().fetchBrowserSessionProfiles()
      return true
    } catch (err) {
      console.error('Failed to switch runtime environment:', err)
      toast.error('Failed to switch servers', {
        description: err instanceof Error ? err.message : String(err)
      })
      return false
    }
  }
})
