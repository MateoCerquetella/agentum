/**
 * Per-profile fleet store: probes `/api/health` against every
 * configured profile so the sidebar / topbar can label rows with the
 * server's real hostname (e.g. `omarchy`, `mateo-mac`) instead of a
 * generic placeholder, and surface unreachable / login-needed states
 * inline.
 *
 * Mirrors the TUI's multi-server boot fanout (`fanout_other_profiles`
 * in `crates/agentum/src/commands/terminal/mod.rs`): one parallel
 * probe per profile, per-server failures degrade to a status flag
 * instead of nuking the whole fleet view.
 */
import { writable, get } from 'svelte/store';
import { profiles, fetchProfile, type Profile } from '$lib/profiles';
import type { Health } from '$lib/api';

export type FleetStatus = 'live' | 'unreachable' | 'login-needed' | 'unknown';

export interface FleetEntry {
  /** Real hostname returned by `/api/health` — empty until we probe. */
  hostname: string;
  version: string;
  status: FleetStatus;
  /** Epoch ms of the last successful probe. 0 means never. */
  lastSuccessAt: number;
}

export const fleet = writable<Record<string, FleetEntry>>({});

const PROBE_TIMEOUT_MS = 2500;

async function probeOne(p: Profile): Promise<[string, FleetEntry]> {
  const fallback: FleetEntry = {
    hostname: '',
    version: '',
    status: 'unknown',
    lastSuccessAt: 0
  };
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), PROBE_TIMEOUT_MS);
    const res = await fetchProfile(p.id, '/api/health', { signal: ctrl.signal });
    clearTimeout(timer);
    if (res.status === 401 || res.status === 403) {
      return [p.id, { ...fallback, status: 'login-needed' }];
    }
    if (!res.ok) {
      return [p.id, { ...fallback, status: 'unreachable' }];
    }
    const body = (await res.json()) as Health;
    return [
      p.id,
      {
        hostname: body.hostname ?? '',
        version: body.version ?? '',
        status: 'live',
        lastSuccessAt: Date.now()
      }
    ];
  } catch {
    return [p.id, { ...fallback, status: 'unreachable' }];
  }
}

/**
 * Re-probe every configured profile in parallel. Cheap (one /api/health
 * round trip per server, 2.5s ceiling); safe to call on a poll interval
 * or in response to profile-list changes.
 */
export async function refreshFleet(): Promise<void> {
  const list = get(profiles);
  const results = await Promise.all(list.map(probeOne));
  const next: Record<string, FleetEntry> = {};
  for (const [id, entry] of results) next[id] = entry;
  fleet.set(next);
}

/**
 * Display label for a profile, preferring its real hostname over the
 * user-supplied label. Loopback (empty baseUrl) always renders as the
 * real hostname when we have one — generic "this server" is a poor
 * fleet-view signal once N > 1 servers exist. Named profiles keep
 * their user-chosen label when set, since the user is the source of
 * truth for "what I want to call this server".
 */
export function profileDisplayLabel(p: Profile, entry?: FleetEntry): string {
  const hostname = entry?.hostname?.trim();
  if (!p.baseUrl) return hostname || 'this machine';
  // Named profile — prefer the user-supplied label, but fall back to
  // the hostname / id so "vps" or similar never reads as empty.
  if (p.label && p.label !== p.id) return p.label;
  return hostname || p.label || p.id;
}

/**
 * Short host fragment (without scheme/path) for display under the
 * primary label. `''` when the profile uses the page origin — that
 * case carries no extra signal.
 */
export function profileHostHint(p: Profile): string {
  if (!p.baseUrl) return '';
  try {
    return new URL(p.baseUrl).host;
  } catch {
    return p.baseUrl;
  }
}

// Auto-refresh whenever the profile list changes (add/remove/rename).
// The boot path subscribes once in the root layout; this keeps the
// store coherent without per-component plumbing.
let lastSig = '';
profiles.subscribe((list) => {
  const sig = list.map((p) => `${p.id}|${p.baseUrl}|${p.token ? '1' : '0'}`).join(',');
  if (sig === lastSig) return;
  lastSig = sig;
  // Fire-and-forget: errors are captured per-profile inside probeOne.
  void refreshFleet();
});
