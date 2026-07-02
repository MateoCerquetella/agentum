import type { ITheme } from '@xterm/xterm'
import { getBuiltinTheme } from './terminal-theme'

// Full-app color themes (#full-app-color-themes): make picking a color theme
// (Dracula, Catppuccin, Gruvbox…) recolor the ENTIRE app chrome, not just
// terminal panes — VS Code style. It works by deriving the app's ~30 semantic
// design tokens (defined in assets/main.css as `:root` / `.dark`) from a
// terminal palette (an xterm `ITheme`) and setting them as inline CSS variables
// on the document root, which win over the built-in light/dark defaults.
//
// The module is deliberately pure (no React) so the palette→token mapping is
// unit-testable in isolation; the DOM writes go through a small injectable root.

// localStorage key holding the active app color theme's NAME (a terminal theme
// name), or absent when the app uses its built-in light/dark chrome. Client-only
// on purpose: it's a visual preference, mirrored like `agentum-theme` so the
// first boot paint matches (see main.tsx).
export const APP_COLOR_THEME_STORAGE_KEY = 'agentum-app-color-theme'

// Marks the document root while a color theme is active, so callers/tests can
// see at a glance whether the built-in appearance owns the theme or we do.
export const APP_COLOR_THEME_ATTR = 'data-app-color-theme'

// The exact semantic tokens we override. Anything NOT listed (terminal-pane
// titles, git-graph lanes, …) keeps the built-in `.dark`/`.light` default —
// which stays correct because we also set the matching class by luminance.
const OVERRIDDEN_TOKENS = [
  '--background',
  '--foreground',
  '--card',
  '--card-foreground',
  '--popover',
  '--popover-foreground',
  '--primary',
  '--primary-foreground',
  '--secondary',
  '--secondary-foreground',
  '--muted',
  '--muted-foreground',
  '--accent',
  '--accent-foreground',
  '--destructive',
  '--destructive-foreground',
  '--border',
  '--input',
  '--ring',
  '--chart-1',
  '--chart-2',
  '--chart-3',
  '--chart-4',
  '--chart-5',
  '--sidebar',
  '--sidebar-foreground',
  '--sidebar-primary',
  '--sidebar-primary-foreground',
  '--sidebar-accent',
  '--sidebar-accent-foreground',
  '--sidebar-border',
  '--sidebar-ring'
] as const

type Rgb = { r: number; g: number; b: number }

// Parse #rgb / #rrggbb (alpha ignored — chrome tokens are opaque). Returns null
// for anything unparseable so the mapper can fall back to a derived value.
function parseHex(color: string | undefined): Rgb | null {
  const value = color?.trim()
  if (!value) {
    return null
  }
  const match = value.match(/^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/)
  if (!match) {
    return null
  }
  const hex = match[1]
  if (hex.length === 3) {
    return {
      r: Number.parseInt(hex[0] + hex[0], 16),
      g: Number.parseInt(hex[1] + hex[1], 16),
      b: Number.parseInt(hex[2] + hex[2], 16)
    }
  }
  return {
    r: Number.parseInt(hex.slice(0, 2), 16),
    g: Number.parseInt(hex.slice(2, 4), 16),
    b: Number.parseInt(hex.slice(4, 6), 16)
  }
}

function clamp255(n: number): number {
  return Math.max(0, Math.min(255, Math.round(n)))
}

function toHex({ r, g, b }: Rgb): string {
  const hex = (n: number): string => clamp255(n).toString(16).padStart(2, '0')
  return `#${hex(r)}${hex(g)}${hex(b)}`
}

// Linear blend: t=0 → a, t=1 → b.
function mix(a: Rgb, b: Rgb, t: number): Rgb {
  const k = Math.max(0, Math.min(1, t))
  return {
    r: a.r + (b.r - a.r) * k,
    g: a.g + (b.g - a.g) * k,
    b: a.b + (b.b - a.b) * k
  }
}

// WCAG relative luminance (0 = black, 1 = white).
function relativeLuminance({ r, g, b }: Rgb): number {
  const channel = (v: number): number => {
    const s = v / 255
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

function isLight(rgb: Rgb): boolean {
  return relativeLuminance(rgb) > 0.5
}

// Black or near-white text, whichever has the higher WCAG contrast on `bg`.
// A plain luminance threshold misjudges mid-tone accents (e.g. a lavender
// primary reads better with dark text even though it isn't "light"), so compare
// the two contrast ratios directly — the crossover sits near luminance 0.18,
// not 0.5.
function readableForeground(bg: Rgb): Rgb {
  const l = relativeLuminance(bg)
  const contrastWithBlack = (l + 0.05) / 0.05
  const contrastWithWhite = 1.05 / (l + 0.05)
  return contrastWithBlack >= contrastWithWhite ? { r: 16, g: 16, b: 16 } : { r: 255, g: 255, b: 255 }
}

const WHITE: Rgb = { r: 255, g: 255, b: 255 }
const BLACK: Rgb = { r: 0, g: 0, b: 0 }

export type DerivedAppTheme = {
  isDark: boolean
  vars: Record<(typeof OVERRIDDEN_TOKENS)[number], string>
}

/**
 * Map a terminal palette to the app's semantic design tokens.
 *
 * Strategy: use the theme's own authored colors where they exist (background,
 * foreground, `selectionBackground` as an elevated surface, `brightBlack` as
 * muted text, ANSI hues for charts/accents), and derive the rest by blending
 * toward the lighter pole (for surfaces that should "pop") or toward the text
 * colour (for subtle greys). Direction flips for light vs dark backgrounds so a
 * light theme's cards get lighter and its borders get slightly darker, matching
 * how the built-in light/dark tokens are authored.
 */
export function deriveAppThemeVars(theme: ITheme): DerivedAppTheme {
  const bg = parseHex(theme.background) ?? { r: 22, g: 22, b: 25 }
  const fg = parseHex(theme.foreground) ?? { r: 244, g: 244, b: 245 }
  const dark = !isLight(bg)

  // The "raised surface" pole: toward text on dark themes, toward white on light
  // themes (so cards/popovers read as elevated in both).
  const raise = dark ? fg : WHITE
  const elevate = (amount: number): Rgb => mix(bg, raise, amount)
  // The "recessed grey" pole: always a nudge toward the text colour.
  const toward = (amount: number): Rgb => mix(bg, fg, amount)

  const selection = parseHex(theme.selectionBackground)
  const comment = parseHex(theme.brightBlack)

  const primary =
    parseHex(theme.blue) ?? parseHex(theme.brightBlue) ?? parseHex(theme.magenta) ?? fg
  const destructive = parseHex(theme.red) ?? parseHex(theme.brightRed) ?? { r: 239, g: 68, b: 68 }

  const card = dark ? elevate(0.05) : elevate(0.55)
  const popover = dark ? elevate(0.06) : elevate(0.7)
  const secondary = selection ?? toward(dark ? 0.12 : 0.07)
  const muted = toward(dark ? 0.08 : 0.05)
  const mutedForeground = comment ?? mix(fg, bg, 0.4)
  const accent = selection ?? toward(dark ? 0.13 : 0.08)
  const border = toward(dark ? 0.16 : 0.11)
  const input = toward(dark ? 0.13 : 0.1)
  const sidebar = dark ? mix(bg, BLACK, 0.25) : toward(0.03)

  const chart = (name: keyof ITheme, brightName: keyof ITheme, fallback: Rgb): string =>
    toHex(parseHex(theme[name] as string) ?? parseHex(theme[brightName] as string) ?? fallback)

  const vars: Record<(typeof OVERRIDDEN_TOKENS)[number], string> = {
    '--background': toHex(bg),
    '--foreground': toHex(fg),
    '--card': toHex(card),
    '--card-foreground': toHex(fg),
    '--popover': toHex(popover),
    '--popover-foreground': toHex(fg),
    '--primary': toHex(primary),
    '--primary-foreground': toHex(readableForeground(primary)),
    '--secondary': toHex(secondary),
    '--secondary-foreground': toHex(fg),
    '--muted': toHex(muted),
    '--muted-foreground': toHex(mutedForeground),
    '--accent': toHex(accent),
    '--accent-foreground': toHex(fg),
    '--destructive': toHex(destructive),
    '--destructive-foreground': toHex(readableForeground(destructive)),
    '--border': toHex(border),
    '--input': toHex(input),
    '--ring': toHex(primary),
    '--chart-1': chart('green', 'brightGreen', primary),
    '--chart-2': chart('blue', 'brightBlue', primary),
    '--chart-3': chart('yellow', 'brightYellow', primary),
    '--chart-4': chart('magenta', 'brightMagenta', primary),
    '--chart-5': chart('cyan', 'brightCyan', primary),
    '--sidebar': toHex(sidebar),
    '--sidebar-foreground': toHex(mix(fg, bg, 0.15)),
    '--sidebar-primary': toHex(primary),
    '--sidebar-primary-foreground': toHex(readableForeground(primary)),
    '--sidebar-accent': toHex(accent),
    '--sidebar-accent-foreground': toHex(fg),
    '--sidebar-border': toHex(border),
    '--sidebar-ring': toHex(primary)
  }

  return { isDark: dark, vars }
}

// The minimal DOM surface we write through, so tests can inject a fake root.
type ThemeStyle = {
  setProperty: (name: string, value: string) => void
  removeProperty: (name: string) => void
}
type ThemeClassList = {
  toggle: (token: string, force?: boolean) => boolean
}
type ThemeRoot = {
  style: ThemeStyle
  classList: ThemeClassList
  setAttribute: (name: string, value: string) => void
  removeAttribute: (name: string) => void
}

type AppColorThemeOptions = {
  root?: ThemeRoot
}

function resolveRoot(options: AppColorThemeOptions): ThemeRoot {
  return options.root ?? (document.documentElement as unknown as ThemeRoot)
}

/**
 * Paint the whole app in `name`'s palette. Sets the derived tokens as inline CSS
 * variables on the root and flips the `dark`/`light` class to match the theme's
 * luminance (so un-overridden tokens and Tailwind `dark:` utilities stay
 * coherent). Returns false (a no-op) if the name resolves to no known theme, so
 * callers can fall back to the built-in appearance.
 */
export function applyAppColorTheme(name: string, options: AppColorThemeOptions = {}): boolean {
  const theme = getBuiltinTheme(name)
  if (!theme) {
    return false
  }
  const { isDark, vars } = deriveAppThemeVars(theme)
  const root = resolveRoot(options)
  for (const token of OVERRIDDEN_TOKENS) {
    root.style.setProperty(token, vars[token])
  }
  root.classList.toggle('dark', isDark)
  root.classList.toggle('light', !isDark)
  root.setAttribute(APP_COLOR_THEME_ATTR, name)
  return true
}

/**
 * Drop the inline token overrides so the app falls back to its built-in
 * light/dark chrome. Deliberately does NOT touch the `dark`/`light` class — the
 * appearance path (App.tsx / applyDocumentTheme) owns it once no color theme is
 * active. Safe to call when nothing was applied.
 */
export function clearAppColorTheme(options: AppColorThemeOptions = {}): void {
  const root = resolveRoot(options)
  for (const token of OVERRIDDEN_TOKENS) {
    root.style.removeProperty(token)
  }
  root.removeAttribute(APP_COLOR_THEME_ATTR)
}

type ThemeStorage = Pick<Storage, 'getItem'>
type ThemeWritableStorage = Pick<Storage, 'setItem' | 'removeItem'>

// The persisted app color theme name, or null when the app uses built-in
// light/dark. Swallows storage failures so boot/read paths never throw.
export function readAppColorThemeName(storage?: ThemeStorage | null): string | null {
  try {
    const source = storage ?? window.localStorage
    const stored = source.getItem(APP_COLOR_THEME_STORAGE_KEY)
    const trimmed = stored?.trim()
    return trimmed ? trimmed : null
  } catch {
    return null
  }
}

// Persist (name) or forget (null) the active app color theme, mirrored to
// localStorage so main.tsx can re-apply it on the next boot's first paint.
// Swallows storage failures — persistence is best-effort, never fatal.
export function persistAppColorThemeName(
  name: string | null,
  storage?: ThemeWritableStorage | null
): void {
  try {
    const target = storage ?? window.localStorage
    if (name) {
      target.setItem(APP_COLOR_THEME_STORAGE_KEY, name)
    } else {
      target.removeItem(APP_COLOR_THEME_STORAGE_KEY)
    }
  } catch {
    /* ignore storage failures */
  }
}
