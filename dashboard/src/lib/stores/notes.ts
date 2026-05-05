import { writable } from 'svelte/store';
import { api, type Note } from '$lib/api';

interface State {
  loading: boolean;
  error: string | null;
  items: Note[];
}

const initial: State = { loading: false, error: null, items: [] };
export const notes = writable<State>(initial);

export async function loadNotes() {
  notes.update((s) => ({ ...s, loading: true, error: null }));
  try {
    const items = await api.listNotes();
    notes.set({ loading: false, error: null, items });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    notes.update((s) => ({ ...s, loading: false, error: msg }));
  }
}
