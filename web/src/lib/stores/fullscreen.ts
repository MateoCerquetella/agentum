/**
 * Global "chrome-less" toggle: when on, the layout hides the sidebar + topbar
 * and the page body fills the viewport. Useful for terminal-heavy workflows.
 *
 * Persisted in localStorage so refresh keeps your last preference.
 */
import { writable } from 'svelte/store';

const KEY = 'agentum_fullscreen';

function readInitial(): boolean {
  if (typeof localStorage === 'undefined') return false;
  return localStorage.getItem(KEY) === '1';
}

export const fullscreen = writable<boolean>(readInitial());

if (typeof window !== 'undefined') {
  fullscreen.subscribe((on) => {
    try {
      localStorage.setItem(KEY, on ? '1' : '0');
    } catch { /* ignore quota */ }
  });
}

export function toggleFullscreen() {
  fullscreen.update((v) => !v);
}

export function exitFullscreen() {
  fullscreen.set(false);
}
