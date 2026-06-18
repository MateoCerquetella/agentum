import { api } from '@/tauri'
import type { CliInstallStatus } from '../../../../shared/cli-install-types'
import type {
  ComputerUsePermissionSetupResult,
  ComputerUsePermissionStatusResult
} from '../../../../shared/computer-use-permissions-types'
import {
  COMPUTER_USE_SKILL_NAME,
  AGENTUM_CLI_SKILL_NAME,
  buildAgentFeatureSkillInstallCommand
} from '@/lib/agent-feature-install-commands'
import { setOrchestrationSettings } from '@/runtime/agentum-server-client'
import { BROWSER_USE_ENABLED_STORAGE_KEY } from '@/lib/browser-use-setup-state'
import { e2eConfig } from '@/lib/e2e-config'
import { showAgentumCliRegistrationPromptToast } from '@/lib/agent-skill-cli-prerequisite'
import {
  ORCHESTRATION_ENABLED_STORAGE_KEY,
  ORCHESTRATION_SETUP_DISMISSED_STORAGE_KEY,
  notifyOrchestrationSetupStateChanged
} from '@/lib/orchestration-setup-state'
import type { EventProps } from '../../../../shared/telemetry-events'

export type OnboardingFeatureSetupId = 'browserUse' | 'computerUse' | 'orchestration'

export type OnboardingFeatureSetupSelection = Record<OnboardingFeatureSetupId, boolean>

export const DEFAULT_ONBOARDING_FEATURE_SETUP_SELECTION: OnboardingFeatureSetupSelection = {
  browserUse: true,
  computerUse: true,
  orchestration: true
}

export const ONBOARDING_FEATURE_SETUP_IDS: readonly OnboardingFeatureSetupId[] = [
  'browserUse',
  'computerUse',
  'orchestration'
]

// Browser Use and Computer Use still install agent skills; Orchestration does
// NOT — it's an agentum MCP capability toggled server-side (see the orchestration
// handling in `runOnboardingFeatureSetup`), so it contributes no skill to the
// install command.
const FEATURE_SKILL_NAMES: Partial<Record<OnboardingFeatureSetupId, string>> = {
  browserUse: AGENTUM_CLI_SKILL_NAME,
  computerUse: COMPUTER_USE_SKILL_NAME
}

const FEATURE_TELEMETRY_IDS: Record<
  OnboardingFeatureSetupId,
  EventProps<'onboarding_feature_setup_toggled'>['feature']
> = {
  browserUse: 'browser_use',
  computerUse: 'computer_use',
  orchestration: 'orchestration'
}

export type OnboardingFeatureSetupWarning = {
  featureId: OnboardingFeatureSetupId | 'cli' | 'skills'
  message: string
}

export type OnboardingFeatureSetupResult = {
  selectedIds: OnboardingFeatureSetupId[]
  cliTouched: boolean
  skillCommandsCopied: boolean
  skillInstallCommand: string | null
  computerUsePermissionsOpened: boolean
  warnings: OnboardingFeatureSetupWarning[]
}

export type OnboardingFeatureSetupDeps = {
  getCliStatus: () => Promise<CliInstallStatus>
  showCliRegistrationPrompt?: () => Promise<void>
  installCli: () => Promise<CliInstallStatus>
  writeClipboardText: (text: string) => Promise<void>
  getComputerUsePermissionStatus: () => Promise<ComputerUsePermissionStatusResult>
  openComputerUsePermissionSetup: () => Promise<ComputerUsePermissionSetupResult>
  setStorageItem: (key: string, value: string) => void
  removeStorageItem: (key: string) => void
  notifyOrchestrationStateChanged: () => void
  /** Write the server-side orchestration gate (the real on/off switch). */
  setOrchestrationEnabledOnServer: (enabled: boolean) => Promise<void>
}

export function hasSelectedOnboardingFeatureSetup(
  selection: OnboardingFeatureSetupSelection
): boolean {
  return ONBOARDING_FEATURE_SETUP_IDS.some((id) => selection[id])
}

export function selectedOnboardingFeatureSetupIds(
  selection: OnboardingFeatureSetupSelection
): OnboardingFeatureSetupId[] {
  return ONBOARDING_FEATURE_SETUP_IDS.filter((id) => selection[id])
}

export function buildOnboardingFeatureSetupClipboardText(
  selection: OnboardingFeatureSetupSelection
): string | null {
  return buildOnboardingFeatureSetupSkillCommand(selection)
}

export function buildOnboardingFeatureSetupSkillCommand(
  selection: OnboardingFeatureSetupSelection
): string | null {
  const skillNames = selectedOnboardingFeatureSetupIds(selection)
    .map((id) => FEATURE_SKILL_NAMES[id])
    .filter((name): name is string => Boolean(name))
  if (skillNames.length === 0) {
    return null
  }
  return buildAgentFeatureSkillInstallCommand(skillNames)
}

export function onboardingFeatureSetupTelemetryFeature(
  id: OnboardingFeatureSetupId
): EventProps<'onboarding_feature_setup_toggled'>['feature'] {
  return FEATURE_TELEMETRY_IDS[id]
}

export function onboardingFeatureSetupTelemetrySelection(
  selection: OnboardingFeatureSetupSelection
): EventProps<'onboarding_feature_setup_terminal_opened'> {
  return {
    browser_use: selection.browserUse,
    computer_use: selection.computerUse,
    orchestration: selection.orchestration,
    selected_count: selectedOnboardingFeatureSetupIds(selection).length
  }
}

export function onboardingFeatureSetupRunTelemetry(
  selection: OnboardingFeatureSetupSelection,
  result: OnboardingFeatureSetupResult
): EventProps<'onboarding_feature_setup_run'> {
  return {
    ...onboardingFeatureSetupTelemetrySelection(selection),
    cli_touched: result.cliTouched,
    skill_commands_copied: result.skillCommandsCopied,
    skill_install_command_prepared: result.skillInstallCommand !== null,
    computer_use_permissions_opened: result.computerUsePermissionsOpened,
    warning_count: result.warnings.length
  }
}

export function createOnboardingFeatureSetupDeps(): OnboardingFeatureSetupDeps {
  const e2eDeps = getE2EOnboardingFeatureSetupDeps()
  if (e2eDeps) {
    return e2eDeps
  }

  return {
    getCliStatus: () => api.cli.getInstallStatus(),
    showCliRegistrationPrompt: showAgentumCliRegistrationPromptToast,
    installCli: () => api.cli.install(),
    writeClipboardText: (text) => api.ui.writeClipboardText(text),
    getComputerUsePermissionStatus: () => api.computerUsePermissions.getStatus(),
    openComputerUsePermissionSetup: () => api.computerUsePermissions.openSetup(),
    setStorageItem: (key, value) => localStorage.setItem(key, value),
    removeStorageItem: (key) => localStorage.removeItem(key),
    notifyOrchestrationStateChanged: notifyOrchestrationSetupStateChanged,
    setOrchestrationEnabledOnServer: async (enabled) => {
      await setOrchestrationSettings(enabled)
    }
  }
}

function getE2EOnboardingFeatureSetupDeps(): OnboardingFeatureSetupDeps | null {
  if (!e2eConfig.enabled || typeof window === 'undefined') {
    return null
  }
  return (
    (window as unknown as { __onboardingFeatureSetupDeps?: OnboardingFeatureSetupDeps })
      .__onboardingFeatureSetupDeps ?? null
  )
}

export async function runOnboardingFeatureSetup(
  selection: OnboardingFeatureSetupSelection,
  deps: OnboardingFeatureSetupDeps = createOnboardingFeatureSetupDeps()
): Promise<OnboardingFeatureSetupResult> {
  const selectedIds = selectedOnboardingFeatureSetupIds(selection)
  const warnings: OnboardingFeatureSetupWarning[] = []
  let cliTouched = false
  let skillCommandsCopied = false
  const skillInstallCommand = buildOnboardingFeatureSetupSkillCommand(selection)
  let computerUsePermissionsOpened = false

  deps.setStorageItem(BROWSER_USE_ENABLED_STORAGE_KEY, selection.browserUse ? '1' : '0')
  // Orchestration is an MCP capability gated server-side; the localStorage key is
  // only a cache so the Settings toggle paints synchronously. Write the cache +
  // notify first (so the UI is responsive), then push the real flag to the server.
  deps.setStorageItem(ORCHESTRATION_ENABLED_STORAGE_KEY, selection.orchestration ? '1' : '0')
  if (selection.orchestration) {
    deps.removeStorageItem(ORCHESTRATION_SETUP_DISMISSED_STORAGE_KEY)
  }
  deps.notifyOrchestrationStateChanged()
  try {
    await deps.setOrchestrationEnabledOnServer(selection.orchestration)
  } catch (error) {
    warnings.push({ featureId: 'orchestration', message: formatFeatureSetupError(error) })
  }

  if (selectedIds.length === 0) {
    return {
      selectedIds,
      cliTouched,
      skillCommandsCopied,
      skillInstallCommand,
      computerUsePermissionsOpened,
      warnings
    }
  }

  // CLI registration only matters for skill-backed features (Browser Use /
  // Computer Use). Orchestration is an MCP and needs no CLI, so skip this when
  // nothing is being installed.
  if (skillInstallCommand !== null) {
    try {
      const status = await deps.getCliStatus()
      if (!status.supported) {
        warnings.push({
          featureId: 'cli',
          message: status.detail ?? 'Agentum CLI registration is not available on this platform.'
        })
      } else if (status.state !== 'installed' || !status.pathConfigured) {
        await deps.showCliRegistrationPrompt?.()
        const next = await deps.installCli()
        cliTouched = true
        if (next.state !== 'installed') {
          warnings.push({
            featureId: 'cli',
            message: next.detail ?? 'Agentum CLI registration needs attention.'
          })
        } else if (!next.pathConfigured && next.detail) {
          warnings.push({ featureId: 'cli', message: next.detail })
        }
      }
    } catch (error) {
      warnings.push({ featureId: 'cli', message: formatFeatureSetupError(error) })
    }
  }

  if (selection.computerUse) {
    try {
      const status = await deps.getComputerUsePermissionStatus()
      const needsMacPermissions =
        status.platform === 'darwin' &&
        status.permissions.some((permission) => permission.status !== 'granted')
      if (needsMacPermissions) {
        await deps.openComputerUsePermissionSetup()
        computerUsePermissionsOpened = true
      }
    } catch (error) {
      warnings.push({
        featureId: 'computerUse',
        message: formatFeatureSetupError(error)
      })
    }
  }

  skillCommandsCopied = await copySkillCommands(selection, deps, warnings)

  return {
    selectedIds,
    cliTouched,
    skillCommandsCopied,
    skillInstallCommand,
    computerUsePermissionsOpened,
    warnings
  }
}

function formatFeatureSetupError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

async function copySkillCommands(
  selection: OnboardingFeatureSetupSelection,
  deps: OnboardingFeatureSetupDeps,
  warnings: OnboardingFeatureSetupWarning[]
): Promise<boolean> {
  const clipboardText = buildOnboardingFeatureSetupClipboardText(selection)
  if (!clipboardText) {
    return false
  }
  try {
    await deps.writeClipboardText(clipboardText)
    return true
  } catch (error) {
    warnings.push({ featureId: 'skills', message: formatFeatureSetupError(error) })
    return false
  }
}
