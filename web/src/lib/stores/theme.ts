import { writable } from 'svelte/store';

export type Theme = 'terminal-dark' | 'paperlight' | 'system';
const STORAGE_KEY = 'agentum_theme';
const ORDER: Theme[] = ['terminal-dark', 'paperlight', 'system'];
const DEFAULT: Theme = 'terminal-dark';

function readInitial(): Theme {
  if (typeof localStorage === 'undefined') return DEFAULT;
  const v = localStorage.getItem(STORAGE_KEY) as Theme | null;
  return v && ORDER.includes(v) ? v : DEFAULT;
}

export const theme = writable<Theme>(readInitial());

export function applyTheme(t: Theme) {
  if (typeof document !== 'undefined') {
    document.documentElement.dataset.theme = t;
  }
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(STORAGE_KEY, t);
  }
  theme.set(t);
}

export function cycleTheme() {
  let current: Theme = DEFAULT;
  theme.subscribe((t) => (current = t))();
  const next = ORDER[(ORDER.indexOf(current) + 1) % ORDER.length];
  applyTheme(next);
}

export function nextTheme(t: Theme): Theme {
  return ORDER[(ORDER.indexOf(t) + 1) % ORDER.length];
}

export const THEMES = ORDER;
