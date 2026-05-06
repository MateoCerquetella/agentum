import { writable } from 'svelte/store';
import { api, type WatchdogEvent } from '$lib/api';

interface State {
  loading: boolean;
  error: string | null;
  items: WatchdogEvent[];
}

const initial: State = { loading: false, error: null, items: [] };
export const watchdog = writable<State>(initial);

/**
 * Cold-start fetch. Live updates flow through the SSE events client
 * once the backend lands the stream.
 */
export async function loadWatchdog(limit = 50): Promise<void> {
  watchdog.update((s) => ({ ...s, loading: true, error: null }));
  try {
    const items = await api.listWatchdog(limit);
    watchdog.set({ loading: false, error: null, items });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    watchdog.update((s) => ({ ...s, loading: false, error: msg }));
  }
}

/** SSE handler — prepend newest events. */
export function pushWatchdogEvent(ev: WatchdogEvent, cap = 200): void {
  watchdog.update((s) => {
    const items = [ev, ...s.items].slice(0, cap);
    return { ...s, items };
  });
}
