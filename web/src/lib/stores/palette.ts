import { writable } from 'svelte/store';

interface State {
  open: boolean;
  query: string;
}

export const palette = writable<State>({ open: false, query: '' });

export function openPalette() {
  palette.set({ open: true, query: '' });
}

export function closePalette() {
  palette.update((s) => ({ ...s, open: false, query: '' }));
}

export function togglePalette() {
  palette.update((s) => ({ ...s, open: !s.open, query: s.open ? s.query : '' }));
}

export const shortcuts = writable<{ open: boolean }>({ open: false });

export function openShortcuts() {
  shortcuts.set({ open: true });
}
export function closeShortcuts() {
  shortcuts.set({ open: false });
}
