import React, { useEffect, useRef } from 'react'
import { Plus, Search, X } from 'lucide-react'
import { useAppStore } from '@/store'
import { Button } from '@/components/ui/button'
import { useShortcutLabel } from '@/hooks/useShortcutLabel'
import {
  focusOperationalSidebarSearch,
  OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT
} from '@/lib/operational-sidebar-search-focus'
import SidebarWorkspaceOptionsMenu from './SidebarWorkspaceOptionsMenu'

export function HierarchySidebarControls({
  query,
  onQueryChange,
  boardControl,
  onOptionsMenuOpenChange
}: {
  query: string
  onQueryChange: (query: string) => void
  boardControl?: React.ReactNode
  onOptionsMenuOpenChange?: (open: boolean) => void
}): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  const openModal = useAppStore((s) => s.openModal)
  const shortcut = useShortcutLabel('worktree.palette')
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const focus = () => {
      if (focusOperationalSidebarSearch(inputRef.current)) return
      requestAnimationFrame(() => focusOperationalSidebarSearch(inputRef.current))
    }
    window.addEventListener(OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT, focus)
    return () => window.removeEventListener(OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT, focus)
  }, [])

  return (
    <div data-sidebar-hierarchy-controls="" className="border-b border-sidebar-border/70 pb-1.5">
      <div className="mx-2 mb-1.5 mt-1 flex h-8 items-center gap-1.5 rounded-md border border-transparent px-2 text-muted-foreground transition-[background-color,border-color,box-shadow] focus-within:border-sidebar-border focus-within:bg-background/70 focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--sidebar-foreground)_3.5%,transparent)]">
        <Search className="size-3.5 shrink-0" strokeWidth={1.9} />
        <input
          ref={inputRef}
          type="search"
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder="Search"
          aria-label="Search workspaces"
          className="min-w-0 flex-1 border-0 bg-transparent text-[11.5px] text-foreground outline-none placeholder:text-muted-foreground/75"
        />
        {query ? (
          <button
            type="button"
            aria-label="Clear search"
            onClick={() => {
              onQueryChange('')
              inputRef.current?.focus()
            }}
            className="grid size-5 shrink-0 place-items-center rounded text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
          >
            <X className="size-3" />
          </button>
        ) : (
          <kbd className="rounded border border-sidebar-border bg-background/45 px-1 py-px font-mono text-[8px] text-muted-foreground/75">
            {shortcut}
          </kbd>
        )}
      </div>

      <div className="flex h-7 items-center justify-between px-2 pl-3">
        <span className="text-[9.5px] font-bold uppercase tracking-[0.07em] text-muted-foreground/80">
          Workspaces
        </span>
        <div className="flex items-center gap-0.5">
          <SidebarWorkspaceOptionsMenu
            preserveWorkspaceBoardOpen
            onMenuOpenChange={onOptionsMenuOpenChange}
          />
          {boardControl}
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="New workspace"
            disabled={repos.length === 0}
            className="text-foreground/80"
            onClick={() => openModal('new-workspace-composer', { telemetrySource: 'sidebar' })}
          >
            <Plus className="size-3.5" strokeWidth={2.25} />
          </Button>
        </div>
      </div>
    </div>
  )
}
