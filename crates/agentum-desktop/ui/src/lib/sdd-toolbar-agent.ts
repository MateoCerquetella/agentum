import type { TuiAgent } from '../../../shared/types'
import { isTuiAgent } from '../../../shared/tui-agent-config'

/** Resolve toolbar eligibility without conflating requested identity, persisted
 * provisioning truth, and transient process evidence. */
export function resolveSddToolbarAgent(args: {
  sessionTool: string | null | undefined
  requestedAgent?: TuiAgent
  liveAgent: TuiAgent | null
}): TuiAgent | null {
  if (isTuiAgent(args.sessionTool)) {
    return args.sessionTool
  }
  if (args.sessionTool === 'terminal') {
    return args.liveAgent
  }
  // undefined means the pane has not bound / its record is still loading;
  // null means the record fetch failed during a reconnect. Launch intent keeps
  // the toolbar stable through either transient hand-off. An explicit terminal
  // value above remains fail-closed unless live evidence recognizes an agent.
  if (args.sessionTool == null) {
    return args.liveAgent ?? args.requestedAgent ?? null
  }
  return args.liveAgent
}
