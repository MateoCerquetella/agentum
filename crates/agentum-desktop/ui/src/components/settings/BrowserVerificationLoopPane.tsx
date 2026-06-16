import { Globe } from 'lucide-react'
import { Label } from '../ui/label'
import { BROWSER_VERIFICATION_LOOP_SKILL_NAME } from '@/lib/agent-feature-install-commands'
import {
  AGENT_SKILL_CLI_PREREQUISITE_NOTICE,
  ensureAgentumCliAvailableForAgentSkillTerminal
} from '@/lib/agent-skill-cli-prerequisite'
import { BROWSER_VERIFICATION_LOOP_SKILL_INSTALL_COMMAND } from '@/lib/browser-verification-loop-install-command'
import {
  GLOBAL_AGENT_SKILL_SOURCE_KINDS,
  useInstalledAgentSkill
} from '@/hooks/useInstalledAgentSkills'
import { SearchableSetting } from './SearchableSetting'
import { matchesSettingsSearch } from './settings-search'
import { useAppStore } from '../../store'
import { BROWSER_VERIFICATION_LOOP_PANE_SEARCH_ENTRIES } from './browser-verification-loop-search'
import { AgentSkillSetupPanel } from './AgentSkillSetupPanel'

export function BrowserVerificationLoopPane(): React.JSX.Element {
  const searchQuery = useAppStore((s) => s.settingsSearchQuery)
  const show = matchesSettingsSearch(searchQuery, BROWSER_VERIFICATION_LOOP_PANE_SEARCH_ENTRIES)

  const {
    installed,
    loading,
    error,
    refresh
  } = useInstalledAgentSkill(BROWSER_VERIFICATION_LOOP_SKILL_NAME, {
    enabled: true,
    sourceKinds: GLOBAL_AGENT_SKILL_SOURCE_KINDS
  })

  if (!show) {
    return <div />
  }

  return (
    <SearchableSetting
      title="Browser Verification Loop"
      description="Drive the Playwright MCP browser to verify a task list, then post pass/fail to the linked GitHub/Linear issue."
      keywords={BROWSER_VERIFICATION_LOOP_PANE_SEARCH_ENTRIES[0].keywords}
      className="space-y-3 py-2"
    >
      <div className="min-w-0 space-y-0.5">
        <Label>Browser Verification Loop</Label>
        <p className="text-xs text-muted-foreground">
          Install the agentic skill, then launch it in an agent session that has a
          Playwright MCP server in <code>.mcp.json</code> and a linked issue. The agent
          verifies each task in a real browser, captures a screenshot per task as
          evidence (strict — no screenshot, no pass), and the result is posted as a
          comment on the issue. Runs the same locally; remote-host parity ships in 008b.
        </p>
      </div>

      <AgentSkillSetupPanel
        title="Browser verification skill"
        description="Lets an agent verify tasks in a real browser via Playwright MCP and report pass/fail to the linked issue."
        command={BROWSER_VERIFICATION_LOOP_SKILL_INSTALL_COMMAND}
        terminalTitle="Browser verification setup"
        terminalAriaLabel="Browser verification skill install terminal"
        terminalWorktreeId="settings-browser-verification-skill-terminal"
        installed={installed}
        loading={loading}
        error={error}
        icon={<Globe className="size-5" />}
        preInstallNotice={AGENT_SKILL_CLI_PREREQUISITE_NOTICE}
        onBeforeOpenTerminal={async () => {
          await ensureAgentumCliAvailableForAgentSkillTerminal()
        }}
        onRecheck={refresh}
      />

      <p className="text-xs text-muted-foreground">
        Once installed, open a session on the repo and run{' '}
        <code>/browser-verification-loop</code> with the linked issue and a stop cap.
        The loop runs unattended and posts a pass/fail comment with per-task evidence.
      </p>
    </SearchableSetting>
  )
}
