import React, { useCallback, useDeferredValue, useMemo, useState } from 'react'
import { useAppStore } from '@/store'
import {
  CommandDialog,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandItem
} from '@/components/ui/command'
import { getSettingsTargetFromSectionId, useSettingsNavigationMetadata } from '@/hooks/useSettingsNavigationMetadata'
import {
  buildCmdJSettingsResults,
  rankCmdJMiddleResults,
  type CmdJSettingsResult
} from '@/components/cmd-j/palette-results'
import type { SettingsNavTarget } from '@/lib/settings-navigation-types'

// Why: mirror WorktreeJumpPalette's section-id → settings-target mapping so a
// `repo-<id>` row opens that repo's pane while every other row maps 1:1. Kept
// local (a 4-line pure helper) rather than introducing a shared abstraction.
// Cmd+Shift+P — a focused settings command palette. Reuses the proven Cmd+J
// settings path: the single navigation registry (useSettingsNavigationMetadata)
// feeds buildCmdJSettingsResults, so sections removed from the registry (e.g.
// floating-workspace / servers / privacy) are excluded here automatically.
export default function SettingsCommandPalette(): React.JSX.Element | null {
  const visible = useAppStore((s) => s.activeModal === 'settings-command-palette')
  const closeModal = useAppStore((s) => s.closeModal)
  const openSettingsPage = useAppStore((s) => s.openSettingsPage)
  const openSettingsTarget = useAppStore((s) => s.openSettingsTarget)
  const sections = useSettingsNavigationMetadata()

  const [query, setQuery] = useState('')
  const deferredQuery = useDeferredValue(query)

  const allResults = useMemo(() => buildCmdJSettingsResults(sections), [sections])

  const results = useMemo<CmdJSettingsResult[]>(() => {
    const trimmed = deferredQuery.trim()
    if (!trimmed) {
      return [...allResults].sort((a, b) => a.order - b.order)
    }
    // Why: reuse the Cmd+J ranking (settings-only — no quick actions) so typing
    // filters and orders identically to the unified palette.
    return rankCmdJMiddleResults({
      query: trimmed,
      settingsResults: allResults,
      actionResults: []
    }).filter((result): result is CmdJSettingsResult => result.kind === 'settings')
  }, [allResults, deferredQuery])

  const handleOpenChange = useCallback(
    (open: boolean) => {
      if (!open) {
        closeModal()
      }
    },
    [closeModal]
  )

  const handleQueryChange = useCallback((next: string) => {
    setQuery(next)
  }, [])

  const handleSelect = useCallback(
    (result: CmdJSettingsResult) => {
      const target = getSettingsTargetFromSectionId(result.sectionId)
      if (result.targetSectionId) {
        target.sectionId = result.targetSectionId
      }
      // Mirror WorktreeJumpPalette.handleSelectSettings: close the palette, then
      // route Settings to the chosen pane.
      closeModal()
      openSettingsTarget(target)
      openSettingsPage()
    },
    [closeModal, openSettingsPage, openSettingsTarget]
  )

  return (
    <CommandDialog
      open={visible}
      onOpenChange={handleOpenChange}
      shouldFilter={false}
      title="Settings search"
      description="Search settings sections"
      commandProps={{ loop: true }}
    >
      <CommandInput
        placeholder="Search settings..."
        value={query}
        onValueChange={handleQueryChange}
      />
      <CommandList className="max-h-[min(460px,62vh)] p-2">
        {results.length === 0 ? (
          <CommandEmpty className="px-3 py-8 text-center text-sm text-muted-foreground">
            No settings match your search.
          </CommandEmpty>
        ) : (
          results.map((result) => {
            const Icon = result.icon
            return (
              <CommandItem
                key={result.id}
                value={result.id}
                onSelect={() => handleSelect(result)}
                className="group flex cursor-pointer items-center gap-3 rounded-lg border border-transparent px-3 py-2.5 outline-none data-[selected=true]:border-border data-[selected=true]:bg-accent data-[selected=true]:text-foreground"
              >
                <Icon className="size-4 shrink-0 text-muted-foreground/85" aria-hidden="true" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium text-foreground">{result.title}</div>
                  <div className="truncate text-xs text-muted-foreground">{result.description}</div>
                </div>
              </CommandItem>
            )
          })
        )}
      </CommandList>
    </CommandDialog>
  )
}
