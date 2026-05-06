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
}

const ACCENT_KEY = 'agentum_accent';
const DENSITY_KEY = 'agentum_density';

const DEFAULT: Tweaks = { accent: '#f36458', density: 'balanced' };

function readInitial(): Tweaks {
  if (typeof localStorage === 'undefined') return DEFAULT;
  const accent  = localStorage.getItem(ACCENT_KEY) ?? DEFAULT.accent;
  const density = (localStorage.getItem(DENSITY_KEY) as Density) ?? DEFAULT.density;
  return { accent, density };
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
