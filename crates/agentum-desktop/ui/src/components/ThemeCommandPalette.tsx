import React, { useEffect, useMemo, useRef, useState } from 'react'
import { Check, Monitor, Moon, Sun, type LucideIcon } from 'lucide-react'

import { useAppStore } from '@/store'
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from '@/components/ui/command'
import { applyDocumentTheme, type DocumentThemePreference } from '@/lib/document-theme'
import {
  BUILTIN_TERMINAL_THEME_NAMES,
  getSystemPrefersDark,
  getTerminalThemePreview,
  resolveEffectiveTerminalAppearance
} from '@/lib/terminal-theme'

// VS Code's "Preferences: Color Theme" flow, adapted to this app's two theme
// axes: the window appearance (System/Light/Dark) previews live as you arrow and
// reverts on cancel; the terminal colour themes show a swatch and apply on Enter
// (they repaint live panes through the normal settings → lifecycle path). Opened
// from the ⌘K palette's "Color Theme…" entry.

type AppearanceOption = {
  value: DocumentThemePreference
  label: string
  icon: LucideIcon
}

const APPEARANCE_OPTIONS: readonly AppearanceOption[] = [
  { value: 'system', label: 'System', icon: Monitor },
  { value: 'light', label: 'Light', icon: Sun },
  { value: 'dark', label: 'Dark', icon: Moon }
]

const APPEARANCE_PREFIX = 'appearance:'
const TERMINAL_PREFIX = 'terminal:'

function appearanceValue(mode: DocumentThemePreference): string {
  return `${APPEARANCE_PREFIX}${mode}`
}

function terminalValue(name: string): string {
  return `${TERMINAL_PREFIX}${name}`
}

/** A few representative colours from a terminal theme, for a preview swatch. */
function terminalSwatch(name: string): string[] {
  const theme = getTerminalThemePreview(name)
  if (!theme) {
    return []
  }
  return [
    theme.background ?? '#000',
    theme.red ?? '#f00',
    theme.green ?? '#0f0',
    theme.blue ?? '#00f',
    theme.yellow ?? '#ff0'
  ]
}

export default function ThemeCommandPalette(): React.JSX.Element {
  const visible = useAppStore((s) => s.activeModal === 'theme-palette')
  const closeModal = useAppStore((s) => s.closeModal)
  const settings = useAppStore((s) => s.settings)
  const updateSettings = useAppStore((s) => s.updateSettings)

  // The window theme in effect when the picker opened, restored on cancel so a
  // live preview never leaks out as a committed change.
  const originalThemeRef = useRef<DocumentThemePreference>('system')
  const committedRef = useRef(false)
  const [highlighted, setHighlighted] = useState<string>(appearanceValue('system'))

  const currentAppearance: DocumentThemePreference = settings?.theme ?? 'system'

  const terminalContext = useMemo(() => {
    if (!settings) {
      return { slot: 'terminalThemeDark' as const, currentName: '' }
    }
    const effective = resolveEffectiveTerminalAppearance(settings, getSystemPrefersDark())
    const slot =
      effective.mode === 'light' && settings.terminalUseSeparateLightTheme
        ? ('terminalThemeLight' as const)
        : ('terminalThemeDark' as const)
    return { slot, currentName: effective.themeName }
  }, [settings])

  // Capture the starting appearance whenever the picker opens.
  useEffect(() => {
    if (!visible) {
      return
    }
    originalThemeRef.current = currentAppearance
    committedRef.current = false
    setHighlighted(appearanceValue(currentAppearance))
    // Intentionally only re-run on open; currentAppearance is the snapshot.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible])

  const revertAppearancePreview = (): void => {
    applyDocumentTheme(originalThemeRef.current)
  }

  const handleValueChange = (value: string): void => {
    setHighlighted(value)
    if (value.startsWith(APPEARANCE_PREFIX)) {
      // Preview the window theme live as the cursor lands on it.
      applyDocumentTheme(value.slice(APPEARANCE_PREFIX.length) as DocumentThemePreference)
    } else {
      // Off the appearance rows — undo any in-flight window preview.
      revertAppearancePreview()
    }
  }

  const handleOpenChange = (open: boolean): void => {
    if (open) {
      return
    }
    if (!committedRef.current) {
      revertAppearancePreview()
    }
    closeModal()
  }

  const commitAppearance = (mode: DocumentThemePreference): void => {
    committedRef.current = true
    void updateSettings({ theme: mode })
    applyDocumentTheme(mode)
    closeModal()
  }

  const commitTerminalTheme = (name: string): void => {
    committedRef.current = true
    // Selecting a terminal theme must not keep a stray window preview.
    revertAppearancePreview()
    void updateSettings({ [terminalContext.slot]: name })
    closeModal()
  }

  return (
    <CommandDialog
      open={visible}
      onOpenChange={handleOpenChange}
      title="Color Theme"
      description="Preview and switch the app and terminal color themes"
      commandProps={{ value: highlighted, onValueChange: handleValueChange }}
    >
      <CommandInput placeholder="Search color themes…" />
      <CommandList>
        <CommandEmpty>No matching themes.</CommandEmpty>
        <CommandGroup heading="App Appearance — previews as you move">
          {APPEARANCE_OPTIONS.map((option) => {
            const Icon = option.icon
            const selected = currentAppearance === option.value
            return (
              <CommandItem
                key={option.value}
                value={appearanceValue(option.value)}
                keywords={['theme', 'appearance', 'color theme', option.label]}
                onSelect={() => commitAppearance(option.value)}
                className="flex items-center gap-2.5"
              >
                <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                <span className="font-medium">{option.label}</span>
                {selected ? (
                  <Check className="ml-auto size-4 shrink-0 text-primary" aria-label="Current" />
                ) : null}
              </CommandItem>
            )
          })}
        </CommandGroup>
        <CommandGroup
          heading={`Terminal Theme — ${
            terminalContext.slot === 'terminalThemeLight' ? 'light mode' : 'dark mode'
          }`}
        >
          {BUILTIN_TERMINAL_THEME_NAMES.map((name) => {
            const selected = terminalContext.currentName === name
            const swatch = terminalSwatch(name)
            return (
              <CommandItem
                key={name}
                value={terminalValue(name)}
                keywords={['terminal', 'theme', 'color theme', name]}
                onSelect={() => commitTerminalTheme(name)}
                className="flex items-center gap-2.5"
              >
                <span
                  aria-hidden
                  className="flex size-4 shrink-0 items-center overflow-hidden rounded-[3px] border border-border/60"
                  style={{ backgroundColor: swatch[0] }}
                >
                  <span className="flex h-full w-full">
                    {swatch.slice(1).map((color, index) => (
                      <span key={index} className="h-full flex-1" style={{ backgroundColor: color }} />
                    ))}
                  </span>
                </span>
                <span className="truncate font-medium">{name}</span>
                {selected ? (
                  <Check className="ml-auto size-4 shrink-0 text-primary" aria-label="Current" />
                ) : null}
              </CommandItem>
            )
          })}
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  )
}
