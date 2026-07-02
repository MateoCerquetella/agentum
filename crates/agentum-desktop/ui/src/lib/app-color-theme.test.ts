import { describe, expect, it } from 'vitest'
import type { ITheme } from '@xterm/xterm'
import {
  APP_COLOR_THEME_ATTR,
  APP_COLOR_THEME_STORAGE_KEY,
  applyAppColorTheme,
  clearAppColorTheme,
  deriveAppThemeVars,
  readAppColorThemeName
} from './app-color-theme'

const DARK_THEME: ITheme = {
  background: '#282a36',
  foreground: '#f8f8f2',
  selectionBackground: '#44475a',
  brightBlack: '#6272a4',
  red: '#ff5555',
  green: '#50fa7b',
  yellow: '#f1fa8c',
  blue: '#bd93f9',
  magenta: '#ff79c6',
  cyan: '#8be9fd'
}

const LIGHT_THEME: ITheme = {
  background: '#fafafa',
  foreground: '#1a1a1a',
  selectionBackground: '#d0d0d0',
  brightBlack: '#8a8a8a',
  red: '#c0392b',
  blue: '#2b6cb0'
}

// A fake document root capturing style/class/attribute writes.
function createFakeRoot() {
  const style = new Map<string, string>()
  const classes = new Set<string>()
  const attrs = new Map<string, string>()
  return {
    root: {
      style: {
        setProperty: (name: string, value: string) => void style.set(name, value),
        removeProperty: (name: string) => void style.delete(name)
      },
      classList: {
        toggle: (token: string, force?: boolean): boolean => {
          const on = force ?? !classes.has(token)
          if (on) {
            classes.add(token)
          } else {
            classes.delete(token)
          }
          return on
        }
      },
      setAttribute: (name: string, value: string) => void attrs.set(name, value),
      removeAttribute: (name: string) => void attrs.delete(name)
    },
    style,
    classes,
    attrs
  }
}

function fakeStorage(value?: string): Pick<Storage, 'getItem'> {
  return { getItem: (key) => (key === APP_COLOR_THEME_STORAGE_KEY ? (value ?? null) : null) }
}

describe('deriveAppThemeVars', () => {
  it('maps background/foreground straight through and flags a dark theme', () => {
    const { isDark, vars } = deriveAppThemeVars(DARK_THEME)
    expect(isDark).toBe(true)
    expect(vars['--background']).toBe('#282a36')
    expect(vars['--foreground']).toBe('#f8f8f2')
  })

  it('uses the theme accent for primary/ring and red for destructive', () => {
    const { vars } = deriveAppThemeVars(DARK_THEME)
    expect(vars['--primary']).toBe('#bd93f9')
    expect(vars['--ring']).toBe('#bd93f9')
    expect(vars['--destructive']).toBe('#ff5555')
  })

  it('uses the theme-authored selection + comment colors for surfaces/muted text', () => {
    const { vars } = deriveAppThemeVars(DARK_THEME)
    expect(vars['--accent']).toBe('#44475a') // selectionBackground
    expect(vars['--secondary']).toBe('#44475a')
    expect(vars['--muted-foreground']).toBe('#6272a4') // brightBlack (comment)
  })

  it('picks a readable foreground by WCAG contrast, not a naive threshold', () => {
    const { vars } = deriveAppThemeVars(DARK_THEME)
    // Both Dracula blue (#bd93f9) and red (#ff5555) are light enough that dark
    // text wins on contrast (the crossover is ~0.18 luminance, not 0.5).
    expect(vars['--primary-foreground']).toBe('#101010')
    expect(vars['--destructive-foreground']).toBe('#101010')
    // A saturated deep accent flips to white text.
    const deep = deriveAppThemeVars({ ...DARK_THEME, blue: '#0a2a6b' })
    expect(deep.vars['--primary-foreground']).toBe('#ffffff')
  })

  it('classifies a light-background theme as not dark', () => {
    const { isDark, vars } = deriveAppThemeVars(LIGHT_THEME)
    expect(isDark).toBe(false)
    expect(vars['--background']).toBe('#fafafa')
  })

  it('emits valid #rrggbb for every overridden token', () => {
    for (const theme of [DARK_THEME, LIGHT_THEME]) {
      const { vars } = deriveAppThemeVars(theme)
      for (const value of Object.values(vars)) {
        expect(value).toMatch(/^#[0-9a-f]{6}$/)
      }
    }
  })

  it('falls back to sane defaults when the palette is missing fields', () => {
    const { vars } = deriveAppThemeVars({ background: '#000000', foreground: '#ffffff' })
    expect(vars['--background']).toBe('#000000')
    // No blue → primary falls back to foreground; no red → default destructive.
    expect(vars['--primary']).toBe('#ffffff')
    expect(vars['--destructive']).toBe('#ef4444')
  })
})

describe('applyAppColorTheme / clearAppColorTheme', () => {
  it('writes tokens, flips the dark class, and marks the root for a real theme', () => {
    const fake = createFakeRoot()
    const applied = applyAppColorTheme('Dracula', { root: fake.root })
    expect(applied).toBe(true)
    expect(fake.style.get('--background')).toBe('#282a36')
    expect(fake.classes.has('dark')).toBe(true)
    expect(fake.classes.has('light')).toBe(false)
    expect(fake.attrs.get(APP_COLOR_THEME_ATTR)).toBe('Dracula')
  })

  it('is a no-op returning false for an unknown theme name', () => {
    const fake = createFakeRoot()
    const applied = applyAppColorTheme('Not A Real Theme', { root: fake.root })
    expect(applied).toBe(false)
    expect(fake.style.size).toBe(0)
    expect(fake.attrs.has(APP_COLOR_THEME_ATTR)).toBe(false)
  })

  it('clear removes the token overrides + marker but leaves the class alone', () => {
    const fake = createFakeRoot()
    applyAppColorTheme('Dracula', { root: fake.root })
    clearAppColorTheme({ root: fake.root })
    expect(fake.style.size).toBe(0)
    expect(fake.attrs.has(APP_COLOR_THEME_ATTR)).toBe(false)
    // The dark class set on apply is intentionally NOT cleared here.
    expect(fake.classes.has('dark')).toBe(true)
  })
})

describe('readAppColorThemeName', () => {
  it('returns the stored theme name', () => {
    expect(readAppColorThemeName(fakeStorage('Dracula'))).toBe('Dracula')
  })

  it('returns null when unset or blank', () => {
    expect(readAppColorThemeName(fakeStorage())).toBeNull()
    expect(readAppColorThemeName(fakeStorage('  '))).toBeNull()
  })
})
