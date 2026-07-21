import React, { useCallback, useDeferredValue, useMemo, useState } from 'react'
import {
  Columns3,
  MessagesSquare,
  Palette,
  Radar,
  Settings as SettingsIcon,
  type LucideIcon
} from 'lucide-react'

import { useAppStore } from '@/store'
import { useAllWorktrees } from '@/store/selectors'
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from '@/components/ui/command'
import { openBoardSurface } from '@/lib/board-route'
import { activateAndRevealWorktree } from '@/lib/worktree-activation'
import { branchName } from '@/lib/git-utils'
import RepoBadgeLabel from '@/components/repo/RepoBadgeLabel'
import {
  getSettingsTargetFromSectionId,
  useSettingsNavigationMetadata
} from '@/hooks/useSettingsNavigationMetadata'
import {
  buildCmdJSettingsResults,
  rankCmdJMiddleResults,
  type CmdJSettingsResult
} from '@/components/cmd-j/palette-results'

type ViewCommand = {
  id: string
  label: string
  hint: string
  icon: LucideIcon
  run: () => void
}

// Why: shouldFilter={false} (needed so settings results can use the Cmd+J
// ranking) means we filter the other groups ourselves. Every query token must
// appear somewhere in the haystack — forgiving enough for palette typing.
function matchesQuery(query: string, haystack: string): boolean {
  const target = haystack.toLowerCase()
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((token) => target.includes(token))
}

/**
 * Cmd+Shift+P — THE command palette (one surface, VS Code muscle memory):
 * top-level view navigation, agent jumping, the Color Theme picker, and
 * settings search. Absorbed the former Cmd+K "Go to" palette and the separate
 * settings-only palette (#210) — Cmd+K belongs to the terminal (clear pane).
 * It complements the Cmd+J worktree/tab switcher.
 */
export default function CommandPalette(): React.JSX.Element {
  const visible = useAppStore((s) => s.activeModal === 'command-palette')
  const closeModal = useAppStore((s) => s.closeModal)
  const openModal = useAppStore((s) => s.openModal)
  const openActivityPage = useAppStore((s) => s.openActivityPage)
  const openHarnessPage = useAppStore((s) => s.openHarnessPage)
  const openSettingsPage = useAppStore((s) => s.openSettingsPage)
  const openSettingsTarget = useAppStore((s) => s.openSettingsTarget)
  const repos = useAppStore((s) => s.repos)
  const allWorktrees = useAllWorktrees()
  const sections = useSettingsNavigationMetadata()

  const [query, setQuery] = useState('')
  const deferredQuery = useDeferredValue(query)
  const trimmedQuery = deferredQuery.trim()

  const repoMap = useMemo(() => new Map(repos.map((r) => [r.id, r])), [repos])
  const agentItems = useMemo(() => allWorktrees.filter((w) => !w.isArchived), [allWorktrees])
  const allSettingsResults = useMemo(() => buildCmdJSettingsResults(sections), [sections])

  // Why: every action dismisses the palette first so Radix finishes its focus
  // teardown before the destination view (or worktree activation) takes over.
  const go = (fn: () => void) => () => {
    closeModal()
    fn()
  }

  const viewCommands: ViewCommand[] = [
    {
      id: 'view-activity',
      label: 'Mission Control',
      hint: 'Home — every agent you’re running',
      icon: Radar,
      run: go(openActivityPage)
    },
    {
      id: 'view-harness',
      label: 'Chat',
      hint: 'Describe what you want → a spec',
      icon: MessagesSquare,
      run: go(openHarnessPage)
    },
    {
      id: 'view-board',
      label: 'Board',
      hint: 'GitHub / GitLab / Linear issues',
      icon: Columns3,
      // Spec 016 D2: a repo resolves → its hub's Tasks tab; none → the
      // Projects page (the fallback IS the no-git-repo handling — the palette
      // never had a gate to preserve).
      run: go(() => openBoardSurface())
    },
    {
      id: 'view-color-theme',
      label: 'Color Theme…',
      hint: 'Switch app & terminal themes',
      icon: Palette,
      run: go(() => openModal('theme-palette'))
    },
    {
      id: 'view-settings',
      label: 'Settings',
      hint: 'Preferences & integrations',
      icon: SettingsIcon,
      run: go(openSettingsPage)
    }
  ]

  const filteredCommands = trimmedQuery
    ? viewCommands.filter((command) =>
        matchesQuery(trimmedQuery, `${command.label} ${command.hint}`)
      )
    : viewCommands

  const filteredAgents = trimmedQuery
    ? agentItems.filter((worktree) =>
        matchesQuery(
          trimmedQuery,
          `${worktree.displayName} ${branchName(worktree.branch)} ${
            repoMap.get(worktree.repoId)?.displayName ?? ''
          }`
        )
      )
    : agentItems

  // Settings only surface once the user types — reusing the Cmd+J ranking so
  // filtering and ordering match the unified Cmd+J palette exactly.
  const settingsResults = useMemo<CmdJSettingsResult[]>(() => {
    if (!trimmedQuery) {
      return []
    }
    return rankCmdJMiddleResults({
      query: trimmedQuery,
      settingsResults: allSettingsResults,
      actionResults: []
    }).filter((result): result is CmdJSettingsResult => result.kind === 'settings')
  }, [allSettingsResults, trimmedQuery])

  const handleSelectSettings = useCallback(
    (result: CmdJSettingsResult) => {
      const target = getSettingsTargetFromSectionId(result.sectionId)
      if (result.targetSectionId) {
        target.sectionId = result.targetSectionId
      }
      // Mirror WorktreeJumpPalette.handleSelectSettings: close the palette,
      // then route Settings to the chosen pane.
      closeModal()
      openSettingsTarget(target)
      openSettingsPage()
    },
    [closeModal, openSettingsPage, openSettingsTarget]
  )

  const handleOpenChange = useCallback(
    (open: boolean) => {
      if (!open) {
        setQuery('')
        closeModal()
      }
    },
    [closeModal]
  )

  const nothingMatches =
    filteredCommands.length === 0 && filteredAgents.length === 0 && settingsResults.length === 0

  return (
    <CommandDialog
      open={visible}
      onOpenChange={handleOpenChange}
      shouldFilter={false}
      title="Command Palette"
      description="Search commands, agents, and settings"
      commandProps={{ loop: true }}
    >
      <CommandInput
        placeholder="Search commands, agents, settings…"
        value={query}
        onValueChange={setQuery}
      />
      <CommandList>
        {nothingMatches ? <CommandEmpty>No matching commands, agents, or settings.</CommandEmpty> : null}
        {filteredCommands.length > 0 ? (
          <CommandGroup heading="Go to">
            {filteredCommands.map((command) => {
              const Icon = command.icon
              return (
                <CommandItem
                  key={command.id}
                  value={command.id}
                  onSelect={command.run}
                  className="flex items-center gap-2.5"
                >
                  <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                  <span className="font-medium">{command.label}</span>
                  <span className="ml-auto truncate pl-3 text-[12px] text-muted-foreground">
                    {command.hint}
                  </span>
                </CommandItem>
              )
            })}
          </CommandGroup>
        ) : null}
        {filteredAgents.length > 0 ? (
          <CommandGroup heading="Agents">
            {filteredAgents.map((worktree) => {
              const repo = repoMap.get(worktree.repoId)
              const branch = branchName(worktree.branch)
              return (
                <CommandItem
                  key={worktree.id}
                  value={worktree.id}
                  onSelect={go(() => {
                    activateAndRevealWorktree(worktree.id)
                  })}
                  className="flex items-center gap-2.5"
                >
                  <span className="truncate font-medium">{worktree.displayName}</span>
                  <span className="truncate text-[12px] text-muted-foreground">{branch}</span>
                  {repo?.displayName ? (
                    <RepoBadgeLabel
                      name={repo.displayName}
                      color={repo.badgeColor}
                      className="ml-auto max-w-[42%] pl-3 text-[12px] text-muted-foreground"
                      badgeClassName="size-2 rounded-[2px]"
                    />
                  ) : null}
                </CommandItem>
              )
            })}
          </CommandGroup>
        ) : null}
        {settingsResults.length > 0 ? (
          <CommandGroup heading="Settings">
            {settingsResults.map((result) => {
              const Icon = result.icon
              return (
                <CommandItem
                  key={result.id}
                  value={result.id}
                  onSelect={() => handleSelectSettings(result)}
                  className="flex items-center gap-2.5"
                >
                  <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                  <span className="truncate font-medium">{result.title}</span>
                  <span className="ml-auto truncate pl-3 text-[12px] text-muted-foreground">
                    {result.description}
                  </span>
                </CommandItem>
              )
            })}
          </CommandGroup>
        ) : null}
      </CommandList>
    </CommandDialog>
  )
}
