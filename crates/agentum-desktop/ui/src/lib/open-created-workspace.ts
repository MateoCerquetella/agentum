import type {
  TuiAgent,
  WorktreeDefaultTabsLaunch,
  WorktreeSetupLaunch
} from '../../../shared/types'
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
  /** Spec 005 F1 (D2): a gated engine run owns the worktree's agents. Skip all
   *  three plain-delivery paths — the draft-open, the picker prompt stash, and
   *  the issueCommand automation — so exactly one (engine-spawned) agent runs. */
  gatedRun?: boolean
}

/**
 * The plain-delivery decision, extracted pure so the spec 005 "three skips"
 * are unit-testable: with `gatedRun` armed all three paths are suppressed;
 * otherwise today's behavior — launch the selected agent (prompt rides as a
 * draft), stash the prompt for the picker when no agent was selected, and run
 * the issueCommand automation when one was built.
 */
export function planCreatedWorkspaceOpen(opts: {
  gatedRun?: boolean
  agent: TuiAgent | null
  prompt?: string
  hasIssueCommand: boolean
}): { launchAgent: boolean; stashPrompt: boolean; runIssueCommand: boolean } {
  if (opts.gatedRun) {
    return { launchAgent: false, stashPrompt: false, runIssueCommand: false }
  }
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
 * With `gatedRun` (spec 005 F1) the Harness Engine drives the worktree: the
 * activation still runs (repo `setup`/`defaultTabs` are project config, not
 * agents) but every plain delivery is suppressed per `planCreatedWorkspaceOpen`.
 */
export function openCreatedWorkspace(opts: OpenCreatedWorkspaceOptions): void {
  const { worktreeId, agent, setup, defaultTabs, issueCommand, gatedRun } = opts
  const prompt = opts.prompt?.trim() ? opts.prompt : undefined
  const plan = planCreatedWorkspaceOpen({
    ...(gatedRun !== undefined ? { gatedRun } : {}),
    agent,
    ...(prompt !== undefined ? { prompt } : {}),
    hasIssueCommand: Boolean(issueCommand)
  })

  activateAndRevealWorktree(worktreeId, {
    sidebarRevealBehavior: 'auto',
    ...(setup ? { setup } : {}),
    ...(defaultTabs ? { defaultTabs } : {}),
    // Belt-and-braces with the composer-side suppression: a gated run never
    // receives the issueCommand automation (the third skip).
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
