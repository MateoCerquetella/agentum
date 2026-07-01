import React, { useMemo } from 'react'
import {
  Columns3,
  List,
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
import { activateAndRevealWorktree } from '@/lib/worktree-activation'
import { branchName } from '@/lib/git-utils'

type ViewCommand = {
  id: string
  label: string
  hint: string
  icon: LucideIcon
  run: () => void
}

/**
 * Cmd+K command palette (Phase 1 nav shell, #48) — the "jump to any view or
 * agent from anywhere" surface, one of the three rules that keep the app
 * un-trappable (rail always visible, Back + breadcrumb, ⌘K). It complements the
 * Cmd+J worktree/tab switcher by adding top-level view navigation; cmdk handles
 * the fuzzy filtering via each item's `value`.
 */
export default function CommandPalette(): React.JSX.Element {
  const visible = useAppStore((s) => s.activeModal === 'command-palette')
  const closeModal = useAppStore((s) => s.closeModal)
  const openModal = useAppStore((s) => s.openModal)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const openActivityPage = useAppStore((s) => s.openActivityPage)
  const openHarnessPage = useAppStore((s) => s.openHarnessPage)
  const openTaskPage = useAppStore((s) => s.openTaskPage)
  const openSettingsPage = useAppStore((s) => s.openSettingsPage)
  const repos = useAppStore((s) => s.repos)
  const allWorktrees = useAllWorktrees()

  const repoMap = useMemo(() => new Map(repos.map((r) => [r.id, r])), [repos])
  const agentItems = useMemo(
    () => allWorktrees.filter((w) => !w.isArchived),
    [allWorktrees]
  )

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
      run: go(() => openTaskPage())
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

  return (
    <CommandDialog
      open={visible}
      onOpenChange={(open) => {
        if (!open) {
          closeModal()
        }
      }}
      title="Go to…"
      description="Jump to any view or agent"
    >
      <CommandInput placeholder="Go to a view or agent…" />
      <CommandList>
        <CommandEmpty>No matching views or agents.</CommandEmpty>
        <CommandGroup heading="Go to">
          {viewCommands.map((command) => {
            const Icon = command.icon
            return (
              <CommandItem
                key={command.id}
                value={`${command.label} ${command.hint}`}
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
        {agentItems.length > 0 ? (
          <CommandGroup heading="Agents">
            {agentItems.map((worktree) => {
              const repo = repoMap.get(worktree.repoId)
              const branch = branchName(worktree.branch)
              return (
                <CommandItem
                  key={worktree.id}
                  value={`${worktree.displayName} ${branch} ${repo?.displayName ?? ''} ${worktree.id}`}
                  onSelect={go(() => {
                    activateAndRevealWorktree(worktree.id)
                  })}
                  className="flex items-center gap-2.5"
                >
                  <span className="truncate font-medium">{worktree.displayName}</span>
                  <span className="truncate text-[12px] text-muted-foreground">{branch}</span>
                  {repo?.displayName ? (
                    <span className="ml-auto truncate pl-3 text-[12px] text-muted-foreground">
                      {repo.displayName}
                    </span>
                  ) : null}
                </CommandItem>
              )
            })}
          </CommandGroup>
        ) : null}
      </CommandList>
    </CommandDialog>
  )
}
