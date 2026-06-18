import type { JSX } from 'react'
import {
  AGENT_SKILL_CLI_PREREQUISITE_NOTICE,
  ensureAgentumCliAvailableForAgentSkillTerminal
} from '@/lib/agent-skill-cli-prerequisite'
import type { InstalledAgentSkillState } from '@/hooks/useInstalledAgentSkills'
import { AgentSkillSetupPanel } from './AgentSkillSetupPanel'

export function OrchestrationSetupCard(props: {
  compact?: boolean
  terminalHeightPx?: number
  skill: InstalledAgentSkillState
}): JSX.Element {
  const { compact, terminalHeightPx, skill } = props

  const setupPanel = (
    <AgentSkillSetupPanel
      className={compact ? 'w-full max-w-[520px]' : undefined}
      title="Agent Orchestration"
      description="A built-in agentum MCP — agents hand off context and coordinate work through Agentum. No skill to install."
      command=""
      terminalTitle="Orchestration"
      terminalAriaLabel="Orchestration setup"
      terminalWorktreeId="feature-wall-orchestration-skill-terminal"
      installed={skill.installed}
      loading={skill.loading}
      error={skill.error}
      terminalHeightPx={terminalHeightPx}
      preInstallNotice={AGENT_SKILL_CLI_PREREQUISITE_NOTICE}
      onBeforeOpenTerminal={async () => {
        await ensureAgentumCliAvailableForAgentSkillTerminal()
      }}
      showRecheckWhenInstalled={false}
      onRecheck={skill.refresh}
    />
  )

  if (compact) {
    return <div className="flex min-h-24 flex-1 items-center justify-center">{setupPanel}</div>
  }
  return <div className="flex">{setupPanel}</div>
}
