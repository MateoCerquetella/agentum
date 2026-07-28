import { describe, expect, it, vi } from 'vitest'
import type {
  ComputerUsePermissionSetupResult,
  ComputerUsePermissionStatusResult
} from '@/shared/computer-use-permissions-types'
import { BROWSER_USE_ENABLED_STORAGE_KEY } from '@/lib/browser-use-setup-state'
import {
  ORCHESTRATION_ENABLED_STORAGE_KEY,
  ORCHESTRATION_SETUP_DISMISSED_STORAGE_KEY
} from '@/lib/orchestration-setup-state'
import {
  DEFAULT_ONBOARDING_FEATURE_SETUP_SELECTION,
  buildOnboardingFeatureSetupClipboardText,
  onboardingFeatureSetupRunTelemetry,
  onboardingFeatureSetupTelemetryFeature,
  onboardingFeatureSetupTelemetrySelection,
  runOnboardingFeatureSetup,
  type OnboardingFeatureSetupDeps,
  type OnboardingFeatureSetupSelection
} from './onboarding-feature-setup'

const GRANTED_COMPUTER_USE_STATUS: ComputerUsePermissionStatusResult = {
  platform: 'darwin',
  helperAppPath: '/Applications/Agentum Computer Use.app',
  helperUnavailableReason: null,
  permissions: [
    { id: 'accessibility', status: 'granted' },
    { id: 'screenshots', status: 'granted' }
  ]
}

const OPENED_COMPUTER_USE_SETUP: ComputerUsePermissionSetupResult = {
  platform: 'darwin',
  helperAppPath: '/Applications/Agentum.app',
  openedSettings: true,
  launchedHelper: true
}

function createDeps(
  overrides: Partial<OnboardingFeatureSetupDeps> = {}
): OnboardingFeatureSetupDeps & {
  storage: Map<string, string>
  clipboardWrites: string[]
  serverOrchestrationWrites: boolean[]
} {
  const storage = new Map<string, string>()
  const clipboardWrites: string[] = []
  const serverOrchestrationWrites: boolean[] = []
  return {
    storage,
    clipboardWrites,
    serverOrchestrationWrites,
    writeClipboardText: vi.fn(async (text: string) => {
      clipboardWrites.push(text)
    }),
    getComputerUsePermissionStatus: vi.fn(async () => GRANTED_COMPUTER_USE_STATUS),
    openComputerUsePermissionSetup: vi.fn(async () => OPENED_COMPUTER_USE_SETUP),
    setStorageItem: vi.fn((key: string, value: string) => {
      storage.set(key, value)
    }),
    removeStorageItem: vi.fn((key: string) => {
      storage.delete(key)
    }),
    notifyOrchestrationStateChanged: vi.fn(),
    setOrchestrationEnabledOnServer: vi.fn(async (enabled: boolean) => {
      serverOrchestrationWrites.push(enabled)
    }),
    ...overrides
  }
}

describe('onboarding feature setup runner', () => {
  it('defaults every setup item on so first-launch setup is ready to run', () => {
    expect(DEFAULT_ONBOARDING_FEATURE_SETUP_SELECTION).toEqual({
      browserUse: true,
      computerUse: true,
      orchestration: true
    })
  })

  it('never produces a skill install command — every capability ships with the agentum MCP', () => {
    // Browser Use → agentum_browser, Computer Use → agentum_computer, and
    // Orchestration → the messaging/task tools are all auto-wired by agentum's
    // MCP server, so there is nothing to `npx skills add`.
    expect(
      buildOnboardingFeatureSetupClipboardText({
        browserUse: true,
        computerUse: true,
        orchestration: true
      })
    ).toBeNull()
    expect(
      buildOnboardingFeatureSetupClipboardText({
        browserUse: false,
        computerUse: false,
        orchestration: true
      })
    ).toBeNull()
  })

  it('builds privacy-safe telemetry payloads for selected feature setup items', () => {
    const selection: OnboardingFeatureSetupSelection = {
      browserUse: true,
      computerUse: false,
      orchestration: true
    }

    expect(onboardingFeatureSetupTelemetryFeature('browserUse')).toBe('browser_use')
    expect(onboardingFeatureSetupTelemetrySelection(selection)).toEqual({
      browser_use: true,
      computer_use: false,
      orchestration: true,
      selected_count: 2
    })
    expect(
      onboardingFeatureSetupRunTelemetry(selection, {
        selectedIds: ['browserUse', 'orchestration'],
        cliTouched: false,
        skillCommandsCopied: false,
        skillInstallCommand: null,
        computerUsePermissionsOpened: false,
        warnings: []
      })
    ).toEqual({
      browser_use: true,
      computer_use: false,
      orchestration: true,
      selected_count: 2,
      cli_touched: false,
      skill_commands_copied: false,
      skill_install_command_prepared: false,
      computer_use_permissions_opened: false,
      warning_count: 0
    })
  })

  it('enables Browser Use, Computer Use, and Orchestration via the MCP — permissions + server gate, no skill or CLI', async () => {
    const deps = createDeps({
      getComputerUsePermissionStatus: vi.fn(
        async (): Promise<ComputerUsePermissionStatusResult> => ({
          platform: 'darwin',
          helperAppPath: '/Applications/Agentum Computer Use.app',
          helperUnavailableReason: null,
          permissions: [
            { id: 'accessibility', status: 'not-granted' },
            { id: 'screenshots', status: 'granted' }
          ]
        })
      )
    })

    const result = await runOnboardingFeatureSetup(
      { browserUse: true, computerUse: true, orchestration: true },
      deps
    )

    expect(result).toEqual({
      selectedIds: ['browserUse', 'computerUse', 'orchestration'],
      cliTouched: false,
      skillCommandsCopied: false,
      skillInstallCommand: null,
      computerUsePermissionsOpened: true,
      warnings: []
    })
    // No skill to install → no clipboard write.
    expect(deps.clipboardWrites).toEqual([])
    // Computer Use still needs the macOS privacy grants.
    expect(deps.getComputerUsePermissionStatus).toHaveBeenCalledTimes(1)
    expect(deps.openComputerUsePermissionSetup).toHaveBeenCalledTimes(1)
    // Flags + the real orchestration switch (the server-side MCP gate).
    expect(deps.storage.get(BROWSER_USE_ENABLED_STORAGE_KEY)).toBe('1')
    expect(deps.storage.get(ORCHESTRATION_ENABLED_STORAGE_KEY)).toBe('1')
    expect(deps.removeStorageItem).toHaveBeenCalledWith(ORCHESTRATION_SETUP_DISMISSED_STORAGE_KEY)
    expect(deps.notifyOrchestrationStateChanged).toHaveBeenCalledTimes(1)
    expect(deps.serverOrchestrationWrites).toEqual([true])
  })

  it('enables the Orchestration MCP without any skill install, CLI registration, or permissions', async () => {
    const deps = createDeps()
    const selection: OnboardingFeatureSetupSelection = {
      browserUse: false,
      computerUse: false,
      orchestration: true
    }

    const result = await runOnboardingFeatureSetup(selection, deps)

    expect(result.selectedIds).toEqual(['orchestration'])
    expect(result.skillCommandsCopied).toBe(false)
    expect(result.skillInstallCommand).toBeNull()
    expect(result.computerUsePermissionsOpened).toBe(false)
    expect(deps.getComputerUsePermissionStatus).not.toHaveBeenCalled()
    expect(deps.openComputerUsePermissionSetup).not.toHaveBeenCalled()
    expect(deps.storage.get(BROWSER_USE_ENABLED_STORAGE_KEY)).toBe('0')
    expect(deps.storage.get(ORCHESTRATION_ENABLED_STORAGE_KEY)).toBe('1')
    // The real switch: the server-side orchestration gate is turned on.
    expect(deps.serverOrchestrationWrites).toEqual([true])
    expect(deps.clipboardWrites).toEqual([])
  })

  it('clears feature markers and turns the orchestration gate off when nothing is selected', async () => {
    const deps = createDeps()

    const result = await runOnboardingFeatureSetup(
      { browserUse: false, computerUse: false, orchestration: false },
      deps
    )

    expect(result).toEqual({
      selectedIds: [],
      cliTouched: false,
      skillCommandsCopied: false,
      skillInstallCommand: null,
      computerUsePermissionsOpened: false,
      warnings: []
    })
    expect(deps.storage.get(BROWSER_USE_ENABLED_STORAGE_KEY)).toBe('0')
    expect(deps.storage.get(ORCHESTRATION_ENABLED_STORAGE_KEY)).toBe('0')
    expect(deps.serverOrchestrationWrites).toEqual([false])
    expect(deps.getComputerUsePermissionStatus).not.toHaveBeenCalled()
    expect(deps.clipboardWrites).toEqual([])
  })

  it('reports a warning when the server orchestration write fails but still finishes setup', async () => {
    const deps = createDeps({
      setOrchestrationEnabledOnServer: vi.fn(async () => {
        throw new Error('server unreachable')
      })
    })

    const result = await runOnboardingFeatureSetup(
      { browserUse: false, computerUse: false, orchestration: true },
      deps
    )

    expect(result.selectedIds).toEqual(['orchestration'])
    expect(result.warnings).toEqual([{ featureId: 'orchestration', message: 'server unreachable' }])
  })
})
