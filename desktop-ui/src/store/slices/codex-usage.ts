import type { StateCreator } from 'zustand'
import type {
  CodexUsageBreakdownRow,
  CodexUsageDailyPoint,
  CodexUsageRange,
  CodexUsageScanState,
  CodexUsageScope,
  CodexUsageSessionRow,
  CodexUsageSummary
} from '../../../../shared/codex-usage-types'
import type { AppState } from '../types'

export type CodexUsageSlice = {
  codexUsageScope: CodexUsageScope
  codexUsageRange: CodexUsageRange
  codexUsageScanState: CodexUsageScanState | null
  codexUsageSummary: CodexUsageSummary | null
  codexUsageDaily: CodexUsageDailyPoint[]
  codexUsageModelBreakdown: CodexUsageBreakdownRow[]
  codexUsageProjectBreakdown: CodexUsageBreakdownRow[]
  codexUsageRecentSessions: CodexUsageSessionRow[]
  setCodexUsageEnabled: (enabled: boolean) => Promise<void>
  setCodexUsageScope: (scope: CodexUsageScope) => Promise<void>
  setCodexUsageRange: (range: CodexUsageRange) => Promise<void>
  fetchCodexUsage: (opts?: { forceRefresh?: boolean }) => Promise<void>
  enableCodexUsage: () => Promise<void>
  refreshCodexUsage: () => Promise<void>
}

export const createCodexUsageSlice: StateCreator<AppState, [], [], CodexUsageSlice> = (
  set,
  get
) => ({
  codexUsageScope: 'orca',
  codexUsageRange: '30d',
  codexUsageScanState: null,
  codexUsageSummary: null,
  codexUsageDaily: [],
  codexUsageModelBreakdown: [],
  codexUsageProjectBreakdown: [],
  codexUsageRecentSessions: [],

  setCodexUsageEnabled: async (enabled) => {
    try {
      const nextScanState = (await window.api.codexUsage.setEnabled({
        enabled
      })) as CodexUsageScanState
      set({
        codexUsageScanState: enabled
          ? {
              ...nextScanState,
              isScanning: true,
              lastScanCompletedAt: null,
              lastScanError: null
            }
          : nextScanState,
        codexUsageSummary: null,
        codexUsageDaily: [],
        codexUsageModelBreakdown: [],
        codexUsageProjectBreakdown: [],
        codexUsageRecentSessions: []
      })
      if (enabled) {
        await get().fetchCodexUsage({ forceRefresh: true })
      }
    } catch (error) {
      console.error('Failed to update Codex usage setting:', error)
    }
  },

  setCodexUsageScope: async (scope) => {
    set({ codexUsageScope: scope })
    await get().fetchCodexUsage()
  },

  setCodexUsageRange: async (range) => {
    set({ codexUsageRange: range })
    await get().fetchCodexUsage()
  },

  fetchCodexUsage: async (opts) => {
    try {
      const scanState = (await window.api.codexUsage.getScanState()) as CodexUsageScanState
      const currentScanState = get().codexUsageScanState
      const shouldPreserveLoadingState =
        opts?.forceRefresh === true &&
        currentScanState?.enabled === true &&
        get().codexUsageSummary === null
      set({
        codexUsageScanState: shouldPreserveLoadingState
          ? {
              ...scanState,
              isScanning: true,
              lastScanCompletedAt: null,
              lastScanError: null
            }
          : scanState
      })
      if (!scanState.enabled) {
        return
      }

      const nextScanState = (await window.api.codexUsage.refresh({
        force: opts?.forceRefresh ?? false
      })) as CodexUsageScanState
      const { codexUsageScope, codexUsageRange } = get()

      const [summary, daily, modelBreakdown, projectBreakdown, recentSessions] = await Promise.all([
        window.api.codexUsage.getSummary({
          scope: codexUsageScope,
          range: codexUsageRange
        }) as Promise<CodexUsageSummary>,
        window.api.codexUsage.getDaily({
          scope: codexUsageScope,
          range: codexUsageRange
        }) as Promise<CodexUsageDailyPoint[]>,
        window.api.codexUsage.getBreakdown({
          scope: codexUsageScope,
          range: codexUsageRange,
          kind: 'model'
        }) as Promise<CodexUsageBreakdownRow[]>,
        window.api.codexUsage.getBreakdown({
          scope: codexUsageScope,
          range: codexUsageRange,
          kind: 'project'
        }) as Promise<CodexUsageBreakdownRow[]>,
        window.api.codexUsage.getRecentSessions({
          scope: codexUsageScope,
          range: codexUsageRange,
          limit: 10
        }) as Promise<CodexUsageSessionRow[]>
      ])

      set({
        codexUsageScanState: nextScanState,
        codexUsageSummary: summary,
        codexUsageDaily: daily,
        codexUsageModelBreakdown: modelBreakdown,
        codexUsageProjectBreakdown: projectBreakdown,
        codexUsageRecentSessions: recentSessions
      })
    } catch (error) {
      console.error('Failed to fetch Codex usage:', error)
    }
  },

  enableCodexUsage: async () => {
    await get().setCodexUsageEnabled(true)
  },

  refreshCodexUsage: async () => {
    await get().fetchCodexUsage({ forceRefresh: true })
  }
})
