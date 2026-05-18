import { writable } from 'svelte/store';
import { THEMES, applyThemeVars, findTheme, getDefaultThemeId } from './themes';

/**
 * Visual tweaks the user can set in /settings. Persisted to
 * localStorage so reloads keep the choice; applied to the document
 * root via CSS custom properties so any consumer of --cta or
 * --root-fs picks them up automatically.
 *
 * The design canvas exposes a third tweak `showRail` (bool) but the
 * README marks it internal-only — not surfaced here.
 */

export type Density = 'dense' | 'balanced' | 'spacious';

/** All accents from the design's tweaks panel (`design/app.jsx`). */
export const ACCENTS = [
  { id: 'cta',    label: 'Coral',    hex: '#f36458' },
  { id: 'link',   label: 'Electric', hex: '#0052ef' },
  { id: 'green',  label: 'Neon',     hex: '#19d600' },
  { id: 'amber',  label: 'Amber',    hex: '#ffb454' }
] as const;

export const DENSITIES: Array<{ id: Density; label: string; px: number }> = [
  { id: 'dense',     label: 'Dense',     px: 14 },
  { id: 'balanced',  label: 'Balanced',  px: 15 },
  { id: 'spacious',  label: 'Spacious',  px: 17 }
];

interface Tweaks {
  /** Theme id (see lib/stores/themes.ts). */
  theme: string;
  /** Hex value of the active accent (matches --cta). */
  accent: string;
  density: Density;
  /** Surface a toast for `agent.awaiting_input` events. */
  notifyAwaitingInput: boolean;
  /** Surface a toast for `agent.finished` events. */
  notifyFinished: boolean;
  /** Surface a toast for `session.crashed` events. */
  notifyCrashed: boolean;
  /** Surface a toast for `watchdog.compact` events. */
  notifyCompact: boolean;
  /** Mirror toast events to OS-level Web Notifications when granted. */
  notifyBrowser: boolean;
  /** Hide the host CPU/RAM strip on the dashboard hero. */
  hideHostStrip: boolean;
  /** How many minutes idle before the Stuck panel surfaces a session. */
  stuckMinutes: number;
}

const THEME_KEY = 'agentum_theme';
const ACCENT_KEY = 'agentum_accent';
const DENSITY_KEY = 'agentum_density';
const NOTIFY_AWAITING_KEY = 'agentum_notify_awaiting';
const NOTIFY_FINISHED_KEY = 'agentum_notify_finished';
const NOTIFY_CRASHED_KEY = 'agentum_notify_crashed';
const NOTIFY_COMPACT_KEY = 'agentum_notify_compact';
const NOTIFY_BROWSER_KEY = 'agentum_notify_browser';
const HIDE_HOST_KEY = 'agentum_hide_host';
const STUCK_MIN_KEY = 'agentum_stuck_minutes';

const DEFAULT: Tweaks = {
  theme: getDefaultThemeId(),
  accent: '#f36458',
  density: 'balanced',
  notifyAwaitingInput: true,
  notifyFinished: true,
  notifyCrashed: true,
  notifyCompact: true,
  // On by default — the chime + in-page toast already cover users
  // who deny OS permission, and leaving it off meant the first event
  // never even kicked the permission flow. Permission is requested
  // lazily on the first real event arrival in `events.ts::maybeNotify`,
  // so a passive user who never visits Settings still gets banners
  // once they click Allow on that prompt.
  notifyBrowser: true,
  hideHostStrip: false,
  stuckMinutes: 5
};

function readBool(key: string, fallback: boolean): boolean {
  if (typeof localStorage === 'undefined') return fallback;
  const v = localStorage.getItem(key);
  return v == null ? fallback : v === '1';
}

function readNum(key: string, fallback: number, min: number, max: number): number {
  if (typeof localStorage === 'undefined') return fallback;
  const v = localStorage.getItem(key);
  if (v == null) return fallback;
  const n = Number.parseInt(v, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

function readInitial(): Tweaks {
  if (typeof localStorage === 'undefined') return DEFAULT;
  const themeId = localStorage.getItem(THEME_KEY) ?? DEFAULT.theme;
  const theme = THEMES.some(t => t.id === themeId) ? themeId : DEFAULT.theme;
  const accent  = localStorage.getItem(ACCENT_KEY) ?? DEFAULT.accent;
  const density = (localStorage.getItem(DENSITY_KEY) as Density) ?? DEFAULT.density;
  return {
    theme,
    accent,
    density,
    notifyAwaitingInput: readBool(NOTIFY_AWAITING_KEY, DEFAULT.notifyAwaitingInput),
    notifyFinished:      readBool(NOTIFY_FINISHED_KEY, DEFAULT.notifyFinished),
    notifyCrashed:       readBool(NOTIFY_CRASHED_KEY,  DEFAULT.notifyCrashed),
    notifyCompact:       readBool(NOTIFY_COMPACT_KEY,  DEFAULT.notifyCompact),
    notifyBrowser:       readBool(NOTIFY_BROWSER_KEY,  DEFAULT.notifyBrowser),
    hideHostStrip:       readBool(HIDE_HOST_KEY,       DEFAULT.hideHostStrip),
    stuckMinutes:        readNum(STUCK_MIN_KEY, DEFAULT.stuckMinutes, 1, 120)
  };
}

export const tweaks = writable<Tweaks>(readInitial());

function densityPx(d: Density): number {
  return DENSITIES.find(x => x.id === d)?.px ?? 15;
}

/** Push values to :root and persist to localStorage. Idempotent.
 *  Order matters: apply theme → accent → density, so an explicit
 *  accent always wins over the theme's default --cta. */
export function applyTweaks(t: Tweaks): void {
  if (typeof document !== 'undefined') {
    applyThemeVars(findTheme(t.theme));
    document.documentElement.style.setProperty('--cta', t.accent);
    document.documentElement.style.setProperty('--root-fs', `${densityPx(t.density)}px`);
  }
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(THEME_KEY, t.theme);
    localStorage.setItem(ACCENT_KEY, t.accent);
    localStorage.setItem(DENSITY_KEY, t.density);
  }
}

export function setTheme(id: string): void {
  tweaks.update(t => {
    // When switching themes, snap the accent to the theme's signature
    // CTA so the dashboard reads as the chosen palette out of the box.
    // Users who want a different accent can re-pick after switching.
    const theme = findTheme(id);
    const next = { ...t, theme: theme.id, accent: theme.vars['--cta'] ?? t.accent };
    applyTweaks(next);
    return next;
  });
}

export function setAccent(hex: string): void {
  tweaks.update(t => {
    const next = { ...t, accent: hex };
    applyTweaks(next);
    return next;
  });
}

export function setDensity(d: Density): void {
  tweaks.update(t => {
    const next = { ...t, density: d };
    applyTweaks(next);
    return next;
  });
}

function persistBool(key: string, v: boolean): void {
  if (typeof localStorage !== 'undefined') localStorage.setItem(key, v ? '1' : '0');
}

export function setNotifyAwaitingInput(v: boolean): void {
  tweaks.update(t => ({ ...t, notifyAwaitingInput: v }));
  persistBool(NOTIFY_AWAITING_KEY, v);
}
export function setNotifyFinished(v: boolean): void {
  tweaks.update(t => ({ ...t, notifyFinished: v }));
  persistBool(NOTIFY_FINISHED_KEY, v);
}
export function setNotifyCrashed(v: boolean): void {
  tweaks.update(t => ({ ...t, notifyCrashed: v }));
  persistBool(NOTIFY_CRASHED_KEY, v);
}
export function setNotifyCompact(v: boolean): void {
  tweaks.update(t => ({ ...t, notifyCompact: v }));
  persistBool(NOTIFY_COMPACT_KEY, v);
}
export function setNotifyBrowser(v: boolean): void {
  tweaks.update(t => ({ ...t, notifyBrowser: v }));
  persistBool(NOTIFY_BROWSER_KEY, v);
}
export function setHideHostStrip(v: boolean): void {
  tweaks.update(t => ({ ...t, hideHostStrip: v }));
  persistBool(HIDE_HOST_KEY, v);
}
export function setStuckMinutes(n: number): void {
  const v = Math.max(1, Math.min(120, Math.round(n)));
  tweaks.update(t => ({ ...t, stuckMinutes: v }));
  if (typeof localStorage !== 'undefined') localStorage.setItem(STUCK_MIN_KEY, String(v));
}
