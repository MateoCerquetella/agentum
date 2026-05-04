import { writable } from 'svelte/store';
import { api, type Session } from '$lib/api';

interface State {
  loading: boolean;
  error: string | null;
  items: Session[];
}

const initial: State = { loading: false, error: null, items: [] };
export const sessions = writable<State>(initial);

export async function loadSessions() {
  sessions.update((s) => ({ ...s, loading: true, error: null }));
  try {
    const items = await api.listSessions();
    sessions.set({ loading: false, error: null, items });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    sessions.update((s) => ({ ...s, loading: false, error: msg }));
  }
}
