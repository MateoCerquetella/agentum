import { writable } from 'svelte/store';

export const newSessionOpen = writable(false);

export function openNewSession() {
  newSessionOpen.set(true);
}

export function closeNewSession() {
  newSessionOpen.set(false);
}
