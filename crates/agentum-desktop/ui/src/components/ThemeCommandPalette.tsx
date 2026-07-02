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
  applyAppColorTheme,
  clearAppColorTheme,
  persistAppColorThemeName,
  readAppColorThemeName
} from '@/lib/app-color-theme'
import {
  BUILTIN_TERMINAL_THEME_NAMES,
  getSystemPrefersDark,
  getTerminalThemePreview,
  resolveEffectiveTerminalAppearance
} from '@/lib/terminal-theme'

// VS Code's "Preferences: Color Theme" flow, adapted to this app. Two rows of
// choices, both previewing live as you arrow and reverting on cancel:
//   • App Appearance (System/Light/Dark) — the built-in light/dark chrome.
//   • Color themes (Dracula, Catppuccin, …) — a terminal palette that now paints
//     the WHOLE app (derived chrome tokens) AND the terminal panes. Committing
//     one persists it as the app color theme; picking an Appearance row clears it
//     back to the built-in chrome. Opened from the ⌘K palette's "Color Theme…".

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

  // The theme state in effect when the picker opened, restored on cancel so a
  // live preview never leaks out as a committed change. Two axes: the window
  // appearance and the full-app color theme (a terminal theme name, or null).
  const originalThemeRef = useRef<DocumentThemePreference>('system')
  const originalColorThemeRef = useRef<string | null>(null)
  const committedRef = useRef(false)
  const [highlighted, setHighlighted] = useState<string>(appearanceValue('system'))
  // The committed full-app color theme (drives the checkmarks). Snapshotted on
  // open; preview never mutates it.
  const [activeColorTheme, setActiveColorTheme] = useState<string | null>(null)

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

  // Capture the starting state whenever the picker opens.
  useEffect(() => {
    if (!visible) {
      return
    }
    originalThemeRef.current = currentAppearance
    originalColorThemeRef.current = readAppColorThemeName()
    committedRef.current = false
    setActiveColorTheme(originalColorThemeRef.current)
    setHighlighted(
      originalColorThemeRef.current
        ? terminalValue(originalColorThemeRef.current)
        : appearanceValue(currentAppearance)
    )
    // Intentionally only re-run on open; the values above are the snapshot.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible])

  // Roll the document back to whatever was in effect when the picker opened —
  // the original color theme if there was one, otherwise the built-in appearance.
  const revertPreview = (): void => {
    const originalColor = originalColorThemeRef.current
    if (originalColor && applyAppColorTheme(originalColor)) {
      return
    }
    clearAppColorTheme()
    applyDocumentTheme(originalThemeRef.current)
  }

  const handleValueChange = (value: string): void => {
    setHighlighted(value)
    if (value.startsWith(APPEARANCE_PREFIX)) {
      // Preview the built-in chrome for this appearance (drop any color theme).
      clearAppColorTheme()
      applyDocumentTheme(value.slice(APPEARANCE_PREFIX.length) as DocumentThemePreference)
    } else if (value.startsWith(TERMINAL_PREFIX)) {
      // Preview the whole app painted in this color theme.
      applyAppColorTheme(value.slice(TERMINAL_PREFIX.length))
    } else {
      revertPreview()
    }
  }

  const handleOpenChange = (open: boolean): void => {
    if (open) {
      return
    }
    if (!committedRef.current) {
      revertPreview()
    }
    closeModal()
  }

  // Picking an Appearance row clears any full-app color theme and returns to the
  // built-in light/dark chrome.
  const commitAppearance = (mode: DocumentThemePreference): void => {
    committedRef.current = true
    persistAppColorThemeName(null)
    clearAppColorTheme()
    void updateSettings({ theme: mode })
    applyDocumentTheme(mode)
    closeModal()
  }

  // Picking a color theme paints the whole app in it (persisted) and points the
  // matching terminal slot at the same theme so panes agree.
  const commitColorTheme = (name: string): void => {
    committedRef.current = true
    persistAppColorThemeName(name)
    applyAppColorTheme(name)
    void updateSettings({ [terminalContext.slot]: name })
    closeModal()
  }

  return (
    <CommandDialog
      open={visible}
      onOpenChange={handleOpenChange}
      title="Color Theme"
      description="Preview and switch the whole app's color theme"
      commandProps={{ value: highlighted, onValueChange: handleValueChange }}
    >
      <CommandInput placeholder="Search color themes…" />
      <CommandList>
        <CommandEmpty>No matching themes.</CommandEmpty>
        <CommandGroup heading="App Appearance — built-in light / dark">
          {APPEARANCE_OPTIONS.map((option) => {
            const Icon = option.icon
            const selected = !activeColorTheme && currentAppearance === option.value
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
        <CommandGroup heading="Color Theme — recolors the whole app">
          {BUILTIN_TERMINAL_THEME_NAMES.map((name) => {
            const selected = activeColorTheme === name
            const swatch = terminalSwatch(name)
            return (
              <CommandItem
                key={name}
                value={terminalValue(name)}
                keywords={['terminal', 'theme', 'color theme', name]}
                onSelect={() => commitColorTheme(name)}
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
