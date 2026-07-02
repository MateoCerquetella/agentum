// Why: the icon is a string key (not a lucide component) so this data module
// stays import-light and node-testable; MissionControlPage maps the key to a
// component. Order matters — Agent Orchestration leads (the user's headline
// "soon" capability).
export type MissionControlSoonCard = {
  id: string
  title: string
  description: string
  icon: 'orchestration' | 'schedule' | 'cost'
}

export const MISSION_CONTROL_SOON_CARDS: MissionControlSoonCard[] = [
  {
    id: 'agent-orchestration',
    title: 'Agent Orchestration',
    description:
      'Coordinate multiple agents across worktrees with task hand-offs and decision gates.',
    icon: 'orchestration'
  },
  {
    id: 'scheduled-automations',
    title: 'Scheduled Automations',
    description: 'Run agents and verification gates on a schedule, hands-free.',
    icon: 'schedule'
  },
  {
    id: 'cost-alerts',
    title: 'Cost Alerts',
    description: 'Get notified when token spend crosses a budget you set.',
    icon: 'cost'
  }
]
