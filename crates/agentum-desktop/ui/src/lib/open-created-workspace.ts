import type {
  TuiAgent,
  WorktreeDefaultTabsLaunch,
  WorktreeSetupLaunch
} from '@/shared/types'
import { activateAndRevealWorktree, type IssueCommandLaunch } from './worktree-activation'
import { launchAgentInNewTab } from './launch-agent-in-new-tab'
import { stashPendingSessionPrompt } from './pending-session-prompt'

export type OpenCreatedWorkspaceOptions = {
  worktreeId: string
  /** The agent the user selected in the composer, or `null` when they chose
   *  none (or "Don't pick an agent"). */
  agent: TuiAgent | null
  /** Resolved prompt (linked-issue context and/or typed text) to deliver, if
   *  any. Whitespace-only is treated as absent. */
  prompt?: string
  setup?: WorktreeSetupLaunch
  defaultTabs?: WorktreeDefaultTabsLaunch
  issueCommand?: IssueCommandLaunch
}

/**
 * The plain-delivery decision: launch the selected agent (the prompt rides as
 * a draft), stash the prompt for the picker when no agent was selected, and
 * run issue automation when one was built.
 */
export function planCreatedWorkspaceOpen(opts: {
  agent: TuiAgent | null
  prompt?: string
  hasIssueCommand: boolean
}): { launchAgent: boolean; stashPrompt: boolean; runIssueCommand: boolean } {
  const hasPrompt = Boolean(opts.prompt?.trim())
  return {
    launchAgent: opts.agent !== null,
    stashPrompt: opts.agent === null && hasPrompt,
    runIssueCommand: opts.hasIssueCommand
  }
}

/**
 * Finish a new-workspace creation by opening the worktree the way the user
 * intends.
 *
 * Why: the composer lets the user pick the agent up front. When they did, we
 * open that agent directly — surfacing the "Start a session" picker again would
 * just ask them to re-pick what they already chose. We only fall back to the
 * picker when no agent was selected, so the picker stays the home for the
 * "create now, decide later" path.
 *
 * In both cases we activate + reveal first (which runs repo setup / default
 * tabs / issue automation) with `skipCreatedAgentStartup: true`. That stops the
 * activation fallback from also relaunching the stamped `createdWithAgent`:
 * when an agent was selected we launch it explicitly below (so any prompt rides
 * along as an editable draft, which the bare reopen fallback can't carry); when
 * none was selected we deliberately want the picker, not an auto-launch.
 *
 */
export function openCreatedWorkspace(opts: OpenCreatedWorkspaceOptions): void {
  const { worktreeId, agent, setup, defaultTabs, issueCommand } = opts
  const prompt = opts.prompt?.trim() ? opts.prompt : undefined
  const plan = planCreatedWorkspaceOpen({
    agent,
    ...(prompt !== undefined ? { prompt } : {}),
    hasIssueCommand: Boolean(issueCommand)
  })

  activateAndRevealWorktree(worktreeId, {
    sidebarRevealBehavior: 'auto',
    ...(setup ? { setup } : {}),
    ...(defaultTabs ? { defaultTabs } : {}),
    ...(plan.runIssueCommand && issueCommand ? { issueCommand } : {}),
    skipCreatedAgentStartup: true
  })

  if (plan.launchAgent && agent) {
    // Mirror the WorkspaceAgentLauncher picker's own launch path: draft delivery
    // leaves any prompt editable rather than auto-submitting, matching the UX
    // the user had after picking from the picker — minus the redundant pick.
    launchAgentInNewTab({
      agent,
      worktreeId,
      launchSource: 'sidebar',
      ...(prompt ? { prompt, promptDelivery: 'draft' as const } : {})
    })
  } else if (plan.stashPrompt && prompt) {
    // No agent selected → the picker delivers the prompt as a draft once the
    // user chooses an agent; without this the typed text would be dropped.
    stashPendingSessionPrompt(worktreeId, prompt)
  }
}
