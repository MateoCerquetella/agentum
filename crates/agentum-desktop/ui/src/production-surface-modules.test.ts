// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'

type SurfaceModule = {
  name: string
  exportName: string
  load: () => Promise<Record<string, unknown>>
}

const surfaces: SurfaceModule[] = [
  { name: 'App shell', exportName: 'default', load: () => import('./App') },
  { name: 'Terminal workspace', exportName: 'default', load: () => import('./components/Terminal') },
  { name: 'Terminal pane', exportName: 'default', load: () => import('./components/terminal-pane/TerminalPane') },
  { name: 'Editor pane', exportName: 'default', load: () => import('./components/editor/EditorPanel') },
  { name: 'Browser pane', exportName: 'default', load: () => import('./components/browser-pane/BrowserPane') },
  { name: 'Right sidebar', exportName: 'default', load: () => import('./components/right-sidebar') },
  { name: 'Explorer sidebar', exportName: 'default', load: () => import('./components/right-sidebar/FileExplorer') },
  { name: 'Search sidebar', exportName: 'default', load: () => import('./components/right-sidebar/Search') },
  { name: 'Source Control sidebar', exportName: 'default', load: () => import('./components/right-sidebar/SourceControl') },
  { name: 'Checks sidebar', exportName: 'default', load: () => import('./components/right-sidebar/ChecksPanel') },
  { name: 'Ports sidebar', exportName: 'default', load: () => import('./components/right-sidebar/PortsPanel') },
  { name: 'Board', exportName: 'default', load: () => import('./components/TaskPage') },
  { name: 'Mission Control', exportName: 'default', load: () => import('./components/mission-control/MissionControlPage') },
  { name: 'Projects', exportName: 'default', load: () => import('./components/projects/ProjectsPage') },
  { name: 'Project Hub', exportName: 'default', load: () => import('./components/project-hub/ProjectHubPage') },
  { name: 'Project Specs', exportName: 'default', load: () => import('./components/sdd-v2/SddWorkspaceBar') },
  { name: 'Project Wiki', exportName: 'default', load: () => import('./components/wiki/WikiPage') },
  { name: 'Project Tasks', exportName: 'ProjectTasksPage', load: () => import('./components/project-hub/ProjectTasksPage') },
  { name: 'Project Sessions', exportName: 'ProjectSessionsList', load: () => import('./components/project-hub/ProjectSessionsList') },
  { name: 'Settings shell', exportName: 'default', load: () => import('./components/settings/Settings') },
  { name: 'Agents settings', exportName: 'AgentsPane', load: () => import('./components/settings/AgentsPane') },
  { name: 'Accounts settings', exportName: 'AccountsPane', load: () => import('./components/settings/AccountsPane') },
  { name: 'MCP settings', exportName: 'McpPane', load: () => import('./components/settings/McpPane') },
  { name: 'Orchestration settings', exportName: 'OrchestrationPane', load: () => import('./components/settings/OrchestrationPane') },
  { name: 'Browser verification settings', exportName: 'BrowserVerificationLoopPane', load: () => import('./components/settings/BrowserVerificationLoopPane') },
  { name: 'Computer Use settings', exportName: 'ComputerUsePane', load: () => import('./components/settings/ComputerUsePane') },
  { name: 'Voice settings', exportName: 'VoicePane', load: () => import('./components/settings/VoicePane') },
  { name: 'General settings', exportName: 'GeneralPane', load: () => import('./components/settings/GeneralPane') },
  { name: 'Integrations settings', exportName: 'IntegrationsPane', load: () => import('./components/settings/IntegrationsPane') },
  { name: 'Git settings', exportName: 'GitPane', load: () => import('./components/settings/GitPane') },
  { name: 'Commit-message settings', exportName: 'CommitMessageAiPane', load: () => import('./components/settings/CommitMessageAiPane') },
  { name: 'Task-source settings', exportName: 'TasksPane', load: () => import('./components/settings/TasksPane') },
  { name: 'Terminal settings', exportName: 'TerminalPane', load: () => import('./components/settings/TerminalPane') },
  { name: 'Quick Commands settings', exportName: 'QuickCommandsPane', load: () => import('./components/settings/QuickCommandsPane') },
  { name: 'Browser settings', exportName: 'BrowserPane', load: () => import('./components/settings/BrowserPane') },
  { name: 'Appearance settings', exportName: 'AppearancePane', load: () => import('./components/settings/AppearancePane') },
  { name: 'Input settings', exportName: 'InputPane', load: () => import('./components/settings/InputPane') },
  { name: 'Notification settings', exportName: 'NotificationsPane', load: () => import('./components/settings/NotificationsPane') },
  { name: 'Shortcut settings', exportName: 'ShortcutsPane', load: () => import('./components/settings/ShortcutsPane') },
  { name: 'Stats settings', exportName: 'StatsPane', load: () => import('./components/stats/StatsPane') },
  { name: 'SSH settings', exportName: 'SshPane', load: () => import('./components/settings/SshPane') },
  { name: 'macOS permission settings', exportName: 'DeveloperPermissionsPane', load: () => import('./components/settings/DeveloperPermissionsPane') },
  { name: 'Experimental settings', exportName: 'ExperimentalPane', load: () => import('./components/settings/ExperimentalPane') },
  { name: 'Project settings', exportName: 'RepositoryPane', load: () => import('./components/settings/RepositoryPane') },
  { name: 'Quick Open', exportName: 'default', load: () => import('./components/QuickOpen') },
  { name: 'Workspace switcher', exportName: 'default', load: () => import('./components/WorktreeJumpPalette') },
  { name: 'Command palette', exportName: 'default', load: () => import('./components/CommandPalette') },
  { name: 'Theme palette', exportName: 'default', load: () => import('./components/ThemeCommandPalette') },
  { name: 'New Workspace', exportName: 'default', load: () => import('./components/NewWorkspaceComposerModal') },
  { name: 'Workspace Cleanup', exportName: 'default', load: () => import('./components/workspace-cleanup/WorkspaceCleanupDialog') },
  { name: 'Feature tour', exportName: 'default', load: () => import('./components/feature-wall/FeatureWallModal') },
  { name: 'Feature tips', exportName: 'default', load: () => import('./components/feature-tips/FeatureTipsModal') },
  { name: 'Pet overlay', exportName: 'default', load: () => import('./components/pet/PetOverlay') },
  { name: 'Onboarding', exportName: 'default', load: () => import('./components/onboarding/OnboardingFlow') }
]

describe('production surface module evaluation', () => {
  it.each(surfaces)(
    '$name evaluates with its production export',
    async ({ load, exportName }) => {
      const module = await load()
      expect(module[exportName]).toBeTruthy()
      expect(['function', 'object']).toContain(typeof module[exportName])
    },
    15_000
  )
})
