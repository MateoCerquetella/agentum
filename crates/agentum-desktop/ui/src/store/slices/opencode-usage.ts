import { api } from '@/tauri'
import type { StateCreator } from 'zustand'
import type {
  OpenCodeUsageBreakdownRow,
  OpenCodeUsageDailyPoint,
  OpenCodeUsageRange,
  OpenCodeUsageScanState,
  OpenCodeUsageScope,
  OpenCodeUsageSessionRow,
  OpenCodeUsageSummary
} from '@/shared/opencode-usage-types'
import type { AppState } from '../types'

export type OpenCodeUsageSlice = {
  openCodeUsageScope: OpenCodeUsageScope
  openCodeUsageRange: OpenCodeUsageRange
  openCodeUsageScanState: OpenCodeUsageScanState | null
  openCodeUsageSummary: OpenCodeUsageSummary | null
  openCodeUsageDaily: OpenCodeUsageDailyPoint[]
  openCodeUsageModelBreakdown: OpenCodeUsageBreakdownRow[]
  openCodeUsageProjectBreakdown: OpenCodeUsageBreakdownRow[]
  openCodeUsageRecentSessions: OpenCodeUsageSessionRow[]
  setOpenCodeUsageEnabled: (enabled: boolean) => Promise<void>
  setOpenCodeUsageScope: (scope: OpenCodeUsageScope) => Promise<void>
  setOpenCodeUsageRange: (range: OpenCodeUsageRange) => Promise<void>
  fetchOpenCodeUsage: (opts?: { forceRefresh?: boolean }) => Promise<void>
  enableOpenCodeUsage: () => Promise<void>
  refreshOpenCodeUsage: () => Promise<void>
}

export const createOpenCodeUsageSlice: StateCreator<AppState, [], [], OpenCodeUsageSlice> = (
  set,
  get
) => ({
  openCodeUsageScope: 'agentum',
  openCodeUsageRange: '30d',
  openCodeUsageScanState: null,
  openCodeUsageSummary: null,
  openCodeUsageDaily: [],
  openCodeUsageModelBreakdown: [],
  openCodeUsageProjectBreakdown: [],
  openCodeUsageRecentSessions: [],

  setOpenCodeUsageEnabled: async (enabled) => {
    try {
      const nextScanState = (await api.openCodeUsage.setEnabled({
        enabled
      })) as OpenCodeUsageScanState
      set({
        openCodeUsageScanState: enabled
          ? {
              ...nextScanState,
              isScanning: true,
              lastScanCompletedAt: null,
              lastScanError: null
            }
          : nextScanState,
        openCodeUsageSummary: null,
        openCodeUsageDaily: [],
        openCodeUsageModelBreakdown: [],
        openCodeUsageProjectBreakdown: [],
        openCodeUsageRecentSessions: []
      })
      if (enabled) {
        await get().fetchOpenCodeUsage({ forceRefresh: true })
      }
    } catch (error) {
      console.error('Failed to update OpenCode usage setting:', error)
    }
  },

  setOpenCodeUsageScope: async (scope) => {
    set({ openCodeUsageScope: scope })
    await get().fetchOpenCodeUsage()
  },

  setOpenCodeUsageRange: async (range) => {
    set({ openCodeUsageRange: range })
    await get().fetchOpenCodeUsage()
  },

  fetchOpenCodeUsage: async (opts) => {
    try {
      const scanState = (await api.openCodeUsage.getScanState()) as OpenCodeUsageScanState
      const currentScanState = get().openCodeUsageScanState
      const shouldPreserveLoadingState =
        opts?.forceRefresh === true &&
        currentScanState?.enabled === true &&
        get().openCodeUsageSummary === null
      set({
        openCodeUsageScanState: shouldPreserveLoadingState
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

      const nextScanState = (await api.openCodeUsage.refresh({
        force: opts?.forceRefresh ?? false
      })) as OpenCodeUsageScanState
      const { openCodeUsageScope, openCodeUsageRange } = get()

      const [summary, daily, modelBreakdown, projectBreakdown, recentSessions] = await Promise.all([
        api.openCodeUsage.getSummary({
          scope: openCodeUsageScope,
          range: openCodeUsageRange
        }) as Promise<OpenCodeUsageSummary>,
        api.openCodeUsage.getDaily({
          scope: openCodeUsageScope,
          range: openCodeUsageRange
        }) as Promise<OpenCodeUsageDailyPoint[]>,
        api.openCodeUsage.getBreakdown({
          scope: openCodeUsageScope,
          range: openCodeUsageRange,
          kind: 'model'
        }) as Promise<OpenCodeUsageBreakdownRow[]>,
        api.openCodeUsage.getBreakdown({
          scope: openCodeUsageScope,
          range: openCodeUsageRange,
          kind: 'project'
        }) as Promise<OpenCodeUsageBreakdownRow[]>,
        api.openCodeUsage.getRecentSessions({
          scope: openCodeUsageScope,
          range: openCodeUsageRange,
          limit: 10
        }) as Promise<OpenCodeUsageSessionRow[]>
      ])

      set({
        openCodeUsageScanState: nextScanState,
        openCodeUsageSummary: summary,
        openCodeUsageDaily: daily,
        openCodeUsageModelBreakdown: modelBreakdown,
        openCodeUsageProjectBreakdown: projectBreakdown,
        openCodeUsageRecentSessions: recentSessions
      })
    } catch (error) {
      console.error('Failed to fetch OpenCode usage:', error)
    }
  },

  enableOpenCodeUsage: async () => {
    await get().setOpenCodeUsageEnabled(true)
  },

  refreshOpenCodeUsage: async () => {
    await get().fetchOpenCodeUsage({ forceRefresh: true })
  }
})
