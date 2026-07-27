import { useMemo, useState } from 'react'
import type React from 'react'
import { Check } from 'lucide-react'
import type { GlobalSettings } from '@/shared/types'
import {
  applyAppColorTheme,
  clearAppColorTheme,
  deriveAppThemeVars,
  persistAppColorThemeName,
  readAppColorThemeName
} from '@/lib/app-color-theme'
import {
  BUILTIN_TERMINAL_THEME_NAMES,
  getBuiltinTheme,
  getSystemPrefersDark,
  resolveEffectiveTerminalAppearance
} from '@/lib/terminal-theme'

// #261 — the Settings → Appearance color-theme gallery. Surfaces the full-app
// color themes (previously reachable only through the ⌘⇧P Color Theme palette)
// as LIVE preview cards: each card is a miniature app window painted with the
// theme's derived chrome tokens (deriveAppThemeVars), so you see the sidebar /
// card / accent story before committing. Commit semantics mirror
// ThemeCommandPalette exactly: picking a theme persists it, repaints the app,
// and points the matching terminal slot at the same palette; "Built-in" clears
// back to the plain light/dark chrome owned by the Theme row above.

type MiniPreview = {
  background: string
  sidebar: string
  card: string
  border: string
  foreground: string
  mutedForeground: string
  primary: string
  accent: string
}

function previewFor(name: string): MiniPreview | null {
  const theme = getBuiltinTheme(name)
  if (!theme) return null
  const { vars } = deriveAppThemeVars(theme)
  return {
    background: vars['--background'],
    sidebar: vars['--sidebar'],
    card: vars['--card'],
    border: vars['--border'],
    foreground: vars['--foreground'],
    mutedForeground: vars['--muted-foreground'],
    primary: vars['--primary'],
    accent: vars['--accent']
  }
}

/** A miniature app window: sidebar strip, a "card", two text lines, one primary
 *  chip — enough visual story to judge the theme without applying it. */
function MiniAppWindow({ p }: { p: MiniPreview }): React.JSX.Element {
  return (
    <div
      aria-hidden
      className="flex h-16 w-full overflow-hidden rounded-md border"
      style={{ backgroundColor: p.background, borderColor: p.border }}
    >
      <div className="h-full w-1/4" style={{ backgroundColor: p.sidebar }}>
        <div className="mx-1.5 mt-2 h-1 rounded-full" style={{ backgroundColor: p.accent }} />
        <div
          className="mx-1.5 mt-1 h-1 rounded-full opacity-60"
          style={{ backgroundColor: p.mutedForeground }}
        />
      </div>
      <div className="flex-1 p-1.5">
        <div
          className="h-8 rounded-sm border p-1"
          style={{ backgroundColor: p.card, borderColor: p.border }}
        >
          <div className="h-1 w-3/4 rounded-full" style={{ backgroundColor: p.foreground }} />
          <div
            className="mt-1 h-1 w-1/2 rounded-full opacity-70"
            style={{ backgroundColor: p.mutedForeground }}
          />
        </div>
        <div className="mt-1.5 flex items-center gap-1">
          <div className="h-2 w-8 rounded-full" style={{ backgroundColor: p.primary }} />
          <div className="h-2 w-5 rounded-full" style={{ backgroundColor: p.accent }} />
        </div>
      </div>
    </div>
  )
}

type AppColorThemeGalleryProps = {
  settings: GlobalSettings
  updateSettings: (updates: Partial<GlobalSettings>) => void
  /** Re-apply the built-in System/Dark/Light chrome after clearing a color theme. */
  applyTheme: (theme: 'system' | 'dark' | 'light') => void
}

export function AppColorThemeGallery({
  settings,
  updateSettings,
  applyTheme
}: AppColorThemeGalleryProps): React.JSX.Element {
  const [activeColorTheme, setActiveColorTheme] = useState<string | null>(() =>
    readAppColorThemeName()
  )

  // Which terminal slot follows the app color theme — the same pairing the
  // ⌘⇧P palette commits, so panes and chrome agree wherever you pick from.
  const terminalSlot = useMemo(() => {
    const effective = resolveEffectiveTerminalAppearance(settings, getSystemPrefersDark())
    return effective.mode === 'light' && settings.terminalUseSeparateLightTheme
      ? ('terminalThemeLight' as const)
      : ('terminalThemeDark' as const)
  }, [settings])

  const commitColorTheme = (name: string): void => {
    persistAppColorThemeName(name)
    applyAppColorTheme(name)
    updateSettings({ [terminalSlot]: name })
    setActiveColorTheme(name)
  }

  const commitBuiltin = (): void => {
    persistAppColorThemeName(null)
    clearAppColorTheme()
    applyTheme(settings.theme)
    setActiveColorTheme(null)
  }

  return (
    <div className="grid grid-cols-2 gap-2.5 md:grid-cols-3">
      <button
        type="button"
        onClick={commitBuiltin}
        aria-pressed={activeColorTheme === null}
        className={`flex flex-col gap-1.5 rounded-lg border p-2 text-left transition-colors ${
          activeColorTheme === null
            ? 'border-foreground/40 ring-2 ring-foreground/15'
            : 'border-border hover:border-foreground/25'
        }`}
      >
        <div className="flex h-16 w-full items-center justify-center overflow-hidden rounded-md border border-border bg-background">
          <span className="text-[11px] text-muted-foreground">System / Dark / Light</span>
        </div>
        <span className="flex items-center gap-1.5 text-[12px] font-medium">
          Built-in
          {activeColorTheme === null ? <Check className="size-3 text-primary" /> : null}
        </span>
      </button>
      {BUILTIN_TERMINAL_THEME_NAMES.map((name) => {
        const p = previewFor(name)
        if (!p) return null
        const selected = activeColorTheme === name
        return (
          <button
            key={name}
            type="button"
            onClick={() => commitColorTheme(name)}
            aria-pressed={selected}
            className={`flex flex-col gap-1.5 rounded-lg border p-2 text-left transition-colors ${
              selected
                ? 'border-foreground/40 ring-2 ring-foreground/15'
                : 'border-border hover:border-foreground/25'
            }`}
          >
            <MiniAppWindow p={p} />
            <span className="flex items-center gap-1.5 truncate text-[12px] font-medium">
              <span className="truncate">{name}</span>
              {selected ? <Check className="size-3 flex-none text-primary" /> : null}
            </span>
          </button>
        )
      })}
    </div>
  )
}
