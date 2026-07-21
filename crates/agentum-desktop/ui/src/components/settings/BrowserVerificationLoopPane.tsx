import { useMemo, useState } from 'react'
import { Globe, Play } from 'lucide-react'
import { toast } from 'sonner'
import { Label } from '../ui/label'
import { Button } from '../ui/button'
import { BROWSER_VERIFICATION_LOOP_SKILL_NAME } from '@/lib/agent-feature-install-commands'
import { BROWSER_VERIFICATION_LOOP_SKILL_INSTALL_COMMAND } from '@/lib/browser-verification-loop-install-command'
import {
  GLOBAL_AGENT_SKILL_SOURCE_KINDS,
  useInstalledAgentSkill
} from '@/hooks/useInstalledAgentSkills'
import { sendBracketedPasteToRunningAgent } from '@/lib/agent-paste-draft'
import { useActiveWorktree } from '@/store/selectors'
import { SearchableSetting } from './SearchableSetting'
import { matchesSettingsSearch } from './settings-search'
import { useAppStore } from '../../store'
import { BROWSER_VERIFICATION_LOOP_PANE_SEARCH_ENTRIES } from './browser-verification-loop-search'
import { AgentSkillSetupPanel } from './AgentSkillSetupPanel'

// Compose the launch prompt. The skill (running in the agent's pane) owns the
// browser loop, the strict-evidence rule, the stop cap, and posting; this is the
// issue-aware kickoff the desktop hands it.
function buildLaunchPrompt(args: { workdir: string; issueRef: string; stopCap: number }): string {
  return [
    'Use the browser-verification-loop skill to verify this work in a real browser.',
    `Repo (workdir): ${args.workdir}`,
    `Linked issue: ${args.issueRef}`,
    `Stop cap: ${args.stopCap} tasks/iterations — do not exceed.`,
    'For each task: drive it in the browser via the Playwright MCP server and capture a screenshot as evidence (STRICT — a pass with no screenshot is invalid).',
    'Then post a pass/fail comment to the linked issue (GitHub: via `gh`; Linear: emit the structured result block).',
    'If the Playwright MCP browser is not available, FAIL LOUDLY with the reason and post nothing green.'
  ].join('\n')
}

export function BrowserVerificationLoopPane(): React.JSX.Element {
  const searchQuery = useAppStore((s) => s.settingsSearchQuery)
  const show = matchesSettingsSearch(searchQuery, BROWSER_VERIFICATION_LOOP_PANE_SEARCH_ENTRIES)

  const { installed, loading, error, refresh } = useInstalledAgentSkill(
    BROWSER_VERIFICATION_LOOP_SKILL_NAME,
    { enabled: true, sourceKinds: GLOBAL_AGENT_SKILL_SOURCE_KINDS }
  )

  const activeWorktree = useActiveWorktree()
  const tabsByWorktree = useAppStore((s) => s.tabsByWorktree)
  const activeTabId = useAppStore((s) => s.activeTabId)
  const ptyIdsByTabId = useAppStore((s) => s.ptyIdsByTabId)
  const [stopCap, setStopCap] = useState(25)
  const [launching, setLaunching] = useState(false)

  // The linked issue lives on the active worktree (GitHub number / Linear id).
  const issueRef = useMemo<string | null>(() => {
    if (!activeWorktree) return null
    if (activeWorktree.linkedPR != null) return `GitHub PR #${activeWorktree.linkedPR}`
    if (activeWorktree.linkedIssue != null) return `GitHub issue #${activeWorktree.linkedIssue}`
    if (activeWorktree.linkedLinearIssue != null)
      return `Linear issue ${activeWorktree.linkedLinearIssue}`
    return null
  }, [activeWorktree])

  // Best-effort: the active (or first) tab's first PTY in this worktree is the
  // agent to drive. sendBracketedPasteToRunningAgent no-ops (returns false) if it
  // isn't a ready agent pane, so a wrong guess fails safe with a toast.
  const agentPtyId = useMemo<string | null>(() => {
    if (!activeWorktree) return null
    const tabs = tabsByWorktree[activeWorktree.id] ?? []
    if (tabs.length === 0) return null
    const tab = tabs.find((candidate) => candidate.id === activeTabId) ?? tabs[0]
    return ptyIdsByTabId[tab.id]?.[0] ?? null
  }, [activeWorktree, tabsByWorktree, activeTabId, ptyIdsByTabId])

  const canLaunch = installed && issueRef != null && agentPtyId != null && !launching

  const handleLaunch = async (): Promise<void> => {
    if (!activeWorktree || !agentPtyId || !issueRef) return
    const prompt = buildLaunchPrompt({
      workdir: activeWorktree.path,
      issueRef,
      stopCap
    })
    setLaunching(true)
    try {
      const delivered = await sendBracketedPasteToRunningAgent({
        ptyId: agentPtyId,
        content: prompt
      })
      if (delivered) {
        toast.success('Browser verification loop launched', { description: issueRef })
      } else {
        toast.error('Could not reach a running agent', {
          description: 'Open an agent session in this workspace, then launch again.'
        })
      }
    } catch {
      toast.error('Failed to launch the verification loop')
    } finally {
      setLaunching(false)
    }
  }

  if (!show) {
    return <div />
  }

  const launchHint = !installed
    ? 'Install the skill first.'
    : issueRef == null
      ? 'Link a GitHub or Linear issue to this workspace to target.'
      : agentPtyId == null
        ? 'Open an agent session in this workspace.'
        : `Targets ${issueRef}.`

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
          Install the agentic skill, then launch it against the active workspace&apos;s
          agent. The agent verifies each task in a real browser via Playwright MCP,
          captures a screenshot per task as evidence (strict — no screenshot, no pass),
          and posts the result as a comment on the linked issue. Runs the same locally;
          remote-host parity ships in 008b.
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
        onRecheck={refresh}
      />

      <div className="space-y-2 rounded-md border border-border/60 p-3">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0 space-y-0.5">
            <Label>Launch on the active workspace</Label>
            <p className="text-xs text-muted-foreground">{launchHint}</p>
          </div>
          <Button size="sm" className="gap-1.5" disabled={!canLaunch} onClick={() => void handleLaunch()}>
            <Play className="size-3.5" />
            {launching ? 'Launching…' : 'Launch'}
          </Button>
        </div>
        <div className="flex items-center gap-2">
          <Label htmlFor="bvl-stop-cap" className="text-xs text-muted-foreground">
            Stop cap (max tasks/iterations)
          </Label>
          <input
            id="bvl-stop-cap"
            type="number"
            min={1}
            max={200}
            value={stopCap}
            onChange={(e) => setStopCap(Math.max(1, Math.min(200, Number(e.target.value) || 1)))}
            className="h-7 w-20 rounded-md border border-border bg-background px-2 text-xs"
          />
        </div>
      </div>

      <p className="text-xs text-muted-foreground">
        Prefer to run it by hand? In an agent session on the repo, invoke{' '}
        <code>/browser-verification-loop</code> with the linked issue and a stop cap.
      </p>
    </SearchableSetting>
  )
}
