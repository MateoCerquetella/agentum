import { writable } from 'svelte/store';

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
  /** Hide the host CPU/RAM strip on the dashboard hero. */
  hideHostStrip: boolean;
  /** How many minutes idle before the Stuck panel surfaces a session. */
  stuckMinutes: number;
}

const ACCENT_KEY = 'agentum_accent';
const DENSITY_KEY = 'agentum_density';
const NOTIFY_AWAITING_KEY = 'agentum_notify_awaiting';
const NOTIFY_FINISHED_KEY = 'agentum_notify_finished';
const NOTIFY_CRASHED_KEY = 'agentum_notify_crashed';
const NOTIFY_COMPACT_KEY = 'agentum_notify_compact';
const HIDE_HOST_KEY = 'agentum_hide_host';
const STUCK_MIN_KEY = 'agentum_stuck_minutes';

const DEFAULT: Tweaks = {
  accent: '#f36458',
  density: 'balanced',
  notifyAwaitingInput: true,
  notifyFinished: true,
  notifyCrashed: true,
  notifyCompact: true,
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
  const accent  = localStorage.getItem(ACCENT_KEY) ?? DEFAULT.accent;
  const density = (localStorage.getItem(DENSITY_KEY) as Density) ?? DEFAULT.density;
  return {
    accent,
    density,
    notifyAwaitingInput: readBool(NOTIFY_AWAITING_KEY, DEFAULT.notifyAwaitingInput),
    notifyFinished:      readBool(NOTIFY_FINISHED_KEY, DEFAULT.notifyFinished),
    notifyCrashed:       readBool(NOTIFY_CRASHED_KEY,  DEFAULT.notifyCrashed),
    notifyCompact:       readBool(NOTIFY_COMPACT_KEY,  DEFAULT.notifyCompact),
    hideHostStrip:       readBool(HIDE_HOST_KEY,       DEFAULT.hideHostStrip),
    stuckMinutes:        readNum(STUCK_MIN_KEY, DEFAULT.stuckMinutes, 1, 120)
  };
}

export const tweaks = writable<Tweaks>(readInitial());

function densityPx(d: Density): number {
  return DENSITIES.find(x => x.id === d)?.px ?? 15;
}

/** Push values to :root and persist to localStorage. Idempotent. */
export function applyTweaks(t: Tweaks): void {
  if (typeof document !== 'undefined') {
    document.documentElement.style.setProperty('--cta', t.accent);
    document.documentElement.style.setProperty('--root-fs', `${densityPx(t.density)}px`);
  }
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(ACCENT_KEY, t.accent);
    localStorage.setItem(DENSITY_KEY, t.density);
  }
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
export function setHideHostStrip(v: boolean): void {
  tweaks.update(t => ({ ...t, hideHostStrip: v }));
  persistBool(HIDE_HOST_KEY, v);
}
export function setStuckMinutes(n: number): void {
  const v = Math.max(1, Math.min(120, Math.round(n)));
  tweaks.update(t => ({ ...t, stuckMinutes: v }));
  if (typeof localStorage !== 'undefined') localStorage.setItem(STUCK_MIN_KEY, String(v));
}
