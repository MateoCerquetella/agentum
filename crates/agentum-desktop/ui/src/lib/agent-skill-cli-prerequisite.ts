import { api } from '@/tauri'
import { toast } from 'sonner'
import type { CliInstallStatus } from '../../../shared/cli-install-types'

type EnsureAgentumCliAvailableOptions = {
  onStatusChange?: (status: CliInstallStatus) => void
  registrationPromptDelayMs?: number
}

export const AGENT_SKILL_CLI_PREREQUISITE_NOTICE =
  'Before opening setup, Agentum may show a system prompt to register the Agentum CLI command on PATH.'

export const CLI_PREREQUISITE_REGISTRATION_TOAST = 'Agentum needs to register its CLI on PATH.'
export const CLI_PREREQUISITE_REGISTRATION_TOAST_DESCRIPTION =
  'Approve the system prompt so skill setup can use the Agentum CLI command.'

export function isAgentumCliAvailableOnPath(status: CliInstallStatus | null | undefined): boolean {
  return status?.state === 'installed' && status.pathConfigured
}

export async function ensureAgentumCliAvailableForAgentSkillTerminal({
  onStatusChange,
  registrationPromptDelayMs = 700
}: EnsureAgentumCliAvailableOptions = {}): Promise<CliInstallStatus | null> {
  try {
    const status = await api.cli.getInstallStatus()
    onStatusChange?.(status)

    if (!status.supported) {
      showCliPrerequisiteWarning(status)
      return status
    }

    if (status.state !== 'installed' || !status.pathConfigured) {
      // Why: macOS may immediately show a native authorization prompt, so the
      // user needs app-level context before that OS dialog appears.
      await showAgentumCliRegistrationPromptToast(registrationPromptDelayMs)
      const next = await api.cli.install()
      onStatusChange?.(next)
      showCliPrerequisiteWarning(next)
      return next
    }

    return status
  } catch (error) {
    toast.error(error instanceof Error ? error.message : 'Failed to register the Agentum CLI in PATH.')
    return null
  }
}

export async function showAgentumCliRegistrationPromptToast(delayMs = 700): Promise<void> {
  toast.message(CLI_PREREQUISITE_REGISTRATION_TOAST, {
    description: CLI_PREREQUISITE_REGISTRATION_TOAST_DESCRIPTION
  })
  await delay(delayMs)
}

function delay(ms: number): Promise<void> {
  if (ms <= 0) {
    return Promise.resolve()
  }
  return new Promise((resolve) => window.setTimeout(resolve, ms))
}

function showCliPrerequisiteWarning(status: CliInstallStatus): void {
  if (!status.supported) {
    toast.warning('Agentum CLI registration is unavailable', {
      description: status.detail ?? 'Install the Agentum CLI before running agent skill setup.'
    })
    return
  }

  if (status.state !== 'installed') {
    toast.warning('Agentum CLI registration needs attention', {
      description: status.detail ?? 'Install the Agentum CLI before running agent skill setup.'
    })
    return
  }

  if (!status.pathConfigured) {
    // Why: the skill installer opens a real shell; agents only get the expected
    // Agentum affordances when that shell can resolve the Agentum CLI command.
    toast.warning('Agentum CLI is not visible on PATH yet', {
      description:
        status.detail ?? 'Restart your shell or add the Agentum CLI directory to PATH before setup.'
    })
  }
}
