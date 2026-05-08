/**
 * Tiny indirection so `events.ts` (which avoids importing `api.ts` to
 * keep the bus loader free of REST plumbing) can still produce a
 * profile-aware events WS URL. Re-exports `wsUrl` from `profiles.ts`
 * under a name that's specific to this call site so a future change
 * to the events feed (path, query shape, etc.) only edits one file.
 */
import { wsUrl } from '../profiles';

export function eventsUrlForActiveProfile(): string {
  return wsUrl('/api/events');
}
