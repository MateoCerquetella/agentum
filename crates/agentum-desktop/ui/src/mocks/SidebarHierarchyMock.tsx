import { useMemo, useState } from 'react'
import {
  Bot,
  Boxes,
  ChevronDown,
  CircleHelp,
  Folder,
  FolderPlus,
  Gauge,
  Globe2,
  ListFilter,
  LoaderCircle,
  Monitor,
  MoreHorizontal,
  PanelLeftClose,
  Plus,
  Search,
  Server,
  Settings,
  SlidersHorizontal,
  Sparkles,
  SquareTerminal,
  X
} from 'lucide-react'

type ActivityState = 'working' | 'idle'
type Surface = {
  id: string
  kind: 'agent' | 'terminal' | 'browser'
  name: string
  state: ActivityState
}
type Workspace = {
  id: string
  name: string
  state: ActivityState
  updated: string
  surfaces: Surface[]
}
type Project = { id: string; name: string; workspaces: Workspace[] }
type Host = {
  id: string
  name: string
  kind: 'local' | 'ssh'
  platform: string
  status: 'online' | 'offline' | 'connecting'
  projects: Project[]
  saved?: number
}

const surface = (
  id: string,
  kind: Surface['kind'],
  name: string,
  state: ActivityState = 'working'
): Surface => ({ id, kind, name, state })

const workspace = (projectId: string, index: number, name: string): Workspace => ({
  id: `${projectId}-${index}`,
  name,
  state: index === 2 ? 'idle' : 'working',
  updated: index === 0 ? 'now' : `${index * 4}m`,
  surfaces: [surface(`${projectId}-${index}-agent`, 'agent', index === 1 ? 'Claude' : 'Codex')]
})

const project = (id: string, name: string, workspaceNames: string[]): Project => ({
  id,
  name,
  workspaces: workspaceNames.map((workspaceName, index) => workspace(id, index, workspaceName))
})

const agentumProject = project('local-agentum', 'agentum', [
  'Topbar is disappearing on host switch',
  'Question Orq',
  'Sidebar density pass'
])

agentumProject.workspaces[0] = {
  ...agentumProject.workspaces[0],
  id: 'topbar',
  surfaces: [
    surface('topbar-codex', 'agent', 'Codex'),
    surface('topbar-shell', 'terminal', 'topbar-host-switch', 'idle'),
    surface('topbar-tests', 'terminal', 'Tests'),
    surface('topbar-dev', 'terminal', 'Vite dev server'),
    surface('topbar-preview', 'browser', 'Agentum preview'),
    surface('topbar-qa', 'browser', 'Browser QA', 'idle')
  ]
}

const INITIAL_HOSTS: Host[] = [
  {
    id: 'local',
    name: 'Local',
    kind: 'local',
    platform: 'Darwin 25.2',
    status: 'online',
    projects: [
      agentumProject,
      project('agentum-www', 'agentum-www', ['Hero polish', 'Docs navigation', 'Pricing copy']),
      project('agentum-tui', 'agentum-tui', ['Host shortcuts', 'Session picker', 'Theme parity']),
      project('platform-tools', 'platform-tools', ['Queue recovery', 'Model routing', 'Usage metrics']),
      project('hermes', 'hermes-webui', ['Chat streaming', 'Tool timeline', 'Mobile pass']),
      project('bandely', 'Bandely', ['CI repair', 'Deploy preview', 'Billing events']),
      project('wiki', 'wiki-core', ['Cross linking', 'Tag cleanup', 'Search ranking']),
      project('dotfiles', 'dotfiles', ['Aerospace rules', 'Shell sync', 'Theme tokens'])
    ]
  },
  {
    id: 'developer',
    name: 'Developer',
    kind: 'ssh',
    platform: 'Linux · SSH',
    status: 'online',
    projects: [
      project('infra', 'infra', ['Host hardening', 'Backups', 'Observability']),
      project('deployment', 'deployment', ['Staging sync', 'Release gate', 'Rollback test'])
    ]
  },
  { id: 'freebee', name: 'Freebee', kind: 'ssh', platform: 'SSH', status: 'offline', saved: 13, projects: [] },
  { id: 'omarchy', name: 'Omarchy', kind: 'ssh', platform: 'SSH', status: 'offline', saved: 2, projects: [] }
]

const hostSessionCount = (host: Host): number =>
  host.projects.reduce((count, item) => count + item.workspaces.length, 0)

const stateTotals = (items: ReadonlyArray<{ state: ActivityState }>): { working: number; idle: number } => ({
  working: items.filter((item) => item.state === 'working').length,
  idle: items.filter((item) => item.state === 'idle').length
})

export function SidebarHierarchyMock(): React.JSX.Element {
  const [hosts, setHosts] = useState(INITIAL_HOSTS)
  const [collapsedHosts, setCollapsedHosts] = useState<Set<string>>(() => new Set(['freebee', 'omarchy']))
  const [expandedProjectId, setExpandedProjectId] = useState('local-agentum')
  const [showAllProjectHosts, setShowAllProjectHosts] = useState<Set<string>>(() => new Set())
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState('topbar')
  const [query, setQuery] = useState('')
  const [searchFocused, setSearchFocused] = useState(false)
  const [activeNav, setActiveNav] = useState<'mission' | 'projects'>('projects')

  const visibleHosts = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return hosts
    return hosts.filter((host) => {
      const searchable = [
        host.name,
        ...host.projects.flatMap((item) => [
          item.name,
          ...item.workspaces.flatMap((worktree) => [
            worktree.name,
            ...worktree.surfaces.map((child) => child.name)
          ])
        ])
      ]
      return searchable.join(' ').toLowerCase().includes(needle)
    })
  }, [hosts, query])

  const toggleHost = (hostId: string): void => {
    setCollapsedHosts((current) => {
      const next = new Set(current)
      next.has(hostId) ? next.delete(hostId) : next.add(hostId)
      return next
    })
  }

  const toggleProject = (projectId: string): void => {
    setExpandedProjectId((current) => (current === projectId ? '' : projectId))
  }

  const reconnect = (hostId: string): void => {
    setHosts((current) =>
      current.map((host) => (host.id === hostId ? { ...host, status: 'connecting' } : host))
    )
    window.setTimeout(() => {
      setHosts((current) =>
        current.map((host) => (host.id === hostId ? { ...host, status: 'online' } : host))
      )
    }, 1000)
  }

  const toggleAllProjects = (hostId: string): void => {
    setShowAllProjectHosts((current) => {
      const next = new Set(current)
      next.has(hostId) ? next.delete(hostId) : next.add(hostId)
      return next
    })
  }

  return (
    <main className="stage">
      <section className="window" aria-label="Agentum compact sidebar prototype">
        <header className="titlebar">
          <div className="traffic" aria-hidden><span /><span /><span /></div>
          <button className="icon-button panel-toggle" aria-label="Collapse sidebar"><PanelLeftClose size={16} /></button>
        </header>

        <nav className="primary-nav" aria-label="Primary">
          <button className={activeNav === 'mission' ? 'is-active' : ''} onClick={() => setActiveNav('mission')}><Gauge size={16} />Mission Control</button>
          <button className={activeNav === 'projects' ? 'is-active' : ''} onClick={() => setActiveNav('projects')}><Boxes size={16} />Projects</button>
        </nav>

        <div className={`search ${searchFocused ? 'is-focused' : ''}`}>
          <Search size={14} />
          <input aria-label="Search workspaces" placeholder="Search" value={query} onFocus={() => setSearchFocused(true)} onBlur={() => setSearchFocused(Boolean(query))} onChange={(event) => setQuery(event.target.value)} />
          {query ? <button aria-label="Clear search" onClick={() => setQuery('')}><X size={12} /></button> : <kbd>⌘K</kbd>}
        </div>

        <div className="section-heading">
          <span>Workspaces</span>
          <div>
            <button className="icon-button" aria-label="Filter"><ListFilter size={14} /></button>
            <button className="icon-button" aria-label="Options"><SlidersHorizontal size={14} /></button>
            <button className="icon-button is-strong" aria-label="New workspace"><Plus size={15} /></button>
          </div>
        </div>

        <div className="tree">
          {visibleHosts.map((host) => {
            const hostCollapsed = collapsedHosts.has(host.id)
            const HostIcon = host.kind === 'local' ? Monitor : Server
            const count = host.status === 'offline' ? host.saved ?? 0 : hostSessionCount(host)
            const hostStates = stateTotals(host.projects.flatMap((item) => item.workspaces))
            const showingAllProjects = showAllProjectHosts.has(host.id) || Boolean(query)
            const projectsToShow = showingAllProjects ? host.projects : host.projects.slice(0, 3)
            const hiddenProjects = host.projects.slice(projectsToShow.length)
            return (
              <section className={`host is-${host.status}`} key={host.id}>
                <div className="host-row">
                  <button className="disclosure" aria-label={`${hostCollapsed ? 'Expand' : 'Collapse'} ${host.name}`} onClick={() => toggleHost(host.id)}><ChevronDown className={hostCollapsed ? 'is-collapsed' : ''} size={14} /></button>
                  <HostIcon className="host-icon" size={15} />
                  <button className="host-name" onClick={() => toggleHost(host.id)}><strong>{host.name}</strong><small>{host.status === 'online' ? host.platform : host.status === 'connecting' ? 'Connecting…' : 'Offline'}</small></button>
                  {host.status === 'connecting' ? <LoaderCircle className="spinner" size={14} /> : <div className="host-actions">
                    {host.status === 'offline' ? (
                      <><button className="reconnect" onClick={() => reconnect(host.id)}>Reconnect</button><span className="count is-offline"><i />{count}</span></>
                    ) : (
                      <span className="state-totals host-state-totals">
                        <span title={`${hostStates.working} working`}><i className="is-working" />{hostStates.working}</span>
                        <span title={`${hostStates.idle} idle`}><i className="is-idle" />{hostStates.idle}</span>
                      </span>
                    )}
                  </div>}
                </div>

                {!hostCollapsed && host.status === 'online' && (
                  <div className="project-list">
                    {projectsToShow.map((item) => {
                      const projectExpanded = expandedProjectId === item.id
                      const projectStates = stateTotals(item.workspaces)
                      return (
                        <div className="project" key={item.id}>
                          <div className="project-row">
                            <button className="disclosure" aria-label={`${projectExpanded ? 'Collapse' : 'Expand'} ${item.name}`} onClick={() => toggleProject(item.id)}><ChevronDown className={projectExpanded ? '' : 'is-collapsed'} size={13} /></button>
                            <Folder size={14} />
                            <button className="project-name" onClick={() => toggleProject(item.id)}>{item.name}</button>
                            <span className="state-totals project-states">
                              <span title={`${projectStates.working} working`}><i className="is-working" />{projectStates.working}</span>
                              <span title={`${projectStates.idle} idle`}><i className="is-idle" />{projectStates.idle}</span>
                            </span>
                            <button className="more" aria-label={`More options for ${item.name}`}><MoreHorizontal size={14} /></button>
                          </div>

                          {projectExpanded && (
                            <div className="workspace-list">
                              {item.workspaces.map((worktree) => {
                                const selected = selectedWorkspaceId === worktree.id
                                return (
                                  <div className={`workspace ${selected ? 'is-selected' : ''}`} key={worktree.id}>
                                    <button className="workspace-row" title={worktree.name} onClick={() => setSelectedWorkspaceId(worktree.id)}>
                                      <span className={`status-dot is-${worktree.state}`} />
                                      <strong>{worktree.name}</strong>
                                      {!selected && <time>{worktree.updated}</time>}
                                    </button>
                                    {selected && (
                                      <div className="inline-session-list" aria-label={`${worktree.surfaces.length} sessions`}>
                                        {worktree.surfaces.map((child) => {
                                          const SessionIcon = child.kind === 'browser' ? Globe2 : child.kind === 'agent' ? Bot : SquareTerminal
                                          return (
                                            <button className={`inline-session-row is-${child.kind}`} title={`${child.name} · ${child.state}`} key={child.id}>
                                              <SessionIcon size={12} />
                                              <strong>{child.name}</strong>
                                              <em className={`is-${child.state}`}><i />{child.state === 'working' ? 'Working' : 'Idle'}</em>
                                            </button>
                                          )
                                        })}
                                      </div>
                                    )}
                                  </div>
                                )
                              })}
                            </div>
                          )}
                        </div>
                      )
                    })}
                    {host.projects.length > 3 && !query && (
                      <button className="more-projects" onClick={() => toggleAllProjects(host.id)}>
                        <ChevronDown className={showingAllProjects ? '' : 'is-collapsed'} size={12} />
                        <span>{showingAllProjects ? 'Show fewer projects' : `${hiddenProjects.length} more projects`}</span>
                        {!showingAllProjects && (() => {
                          const hiddenStates = stateTotals(hiddenProjects.flatMap((item) => item.workspaces))
                          return <span className="state-totals hidden-states"><span><i className="is-working" />{hiddenStates.working}</span><span><i className="is-idle" />{hiddenStates.idle}</span></span>
                        })()}
                      </button>
                    )}
                  </div>
                )}
              </section>
            )
          })}
          {!visibleHosts.length && <div className="empty"><Search size={16} />No matches</div>}
        </div>

        <footer>
          <button className="add-project"><FolderPlus size={15} />Add project</button>
          <div><button className="icon-button" aria-label="Help"><CircleHelp size={15} /></button><button className="icon-button" aria-label="Settings"><Settings size={15} /></button></div>
          <button className="usage"><Sparkles size={13} /><span><strong>92%</strong> weekly</span></button>
        </footer>

      </section>
    </main>
  )
}
