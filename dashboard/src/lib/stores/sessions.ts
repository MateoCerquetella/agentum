import { writable, get } from 'svelte/store';
import { type Session, type Status } from '$lib/api';
import { profiles, fetchProfile } from '$lib/profiles';

interface State {
  loading: boolean;
  error: string | null;
  items: Session[];
}

const initial: State = { loading: false, error: null, items: [] };
export const sessions = writable<State>(initial);

/**
 * Load sessions from every configured endpoint in parallel and merge
 * them into one list — the "all in one control plane" view. Each
 * session is tagged with its owning profile id + label so the fleet
 * row can show an endpoint pill and the terminal page can route the
 * WS to the right host. Per-profile failures (unreachable, expired
 * token) degrade to an empty list for that profile; the others still
 * show.
 */
export async function loadSessions(filter?: Status) {
  sessions.update((s) => ({ ...s, loading: true, error: null }));
  const list = get(profiles);
  const qs = filter ? `?status=${encodeURIComponent(filter)}` : '';
  const promises = list.map(async (p) => {
    try {
      const res = await fetchProfile(p.id, `/api/sessions${qs}`);
      if (!res.ok) return [] as Session[];
      const items = (await res.json()) as Session[];
      // Tag with the profile context so downstream renderers don't
      // need to re-derive from the active profile (which would be
      // wrong for any non-active endpoint's sessions).
      return items.map((s) => ({
        ...s,
        profile: p.id,
        profile_label: p.label
      }));
    } catch {
      return [] as Session[];
    }
  });
  try {
    const results = await Promise.all(promises);
    const items: Session[] = results.flat();
    sessions.set({ loading: false, error: null, items });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    sessions.update((s) => ({ ...s, loading: false, error: msg }));
  }
}
