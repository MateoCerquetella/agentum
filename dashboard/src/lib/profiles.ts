/**
 * Named-endpoint profiles for the dashboard.
 *
 * Mirrors the TUI's `profiles.toml` concept: each profile pins a base
 * URL (and the bearer token earned for that endpoint), so a user can
 * switch between local + remote agentum servers from the same browser
 * tab without retyping credentials. The active profile id is held in
 * a tiny in-memory store; pages reactively re-render when it changes.
 *
 * Storage: `localStorage` under two keys:
 *   - `agentum_profiles`  → JSON array of `Profile`
 *   - `agentum_active`    → string profile id, or empty for "same origin"
 *
 * The legacy `agentum_token` key (single-token, pre-profile) is read
 * once on first load and migrated into a `local` profile so existing
 * sessions don't need to log in again after upgrade.
 */
import { writable, get } from 'svelte/store';

export interface Profile {
  /** Stable id used in URLs / localStorage. Must be slug-safe. */
  id: string;
  /** Human-readable label shown in the picker. */
  label: string;
  /**
   * Absolute base URL of the agentum server, e.g.
   * `https://my-vps.example.com:8822`. Empty string ⇒ "same origin
   * as this page" (preserves the old single-server behaviour).
   */
  baseUrl: string;
  /** Bearer token earned for this endpoint. Empty until the user logs in. */
  token: string;
}

const PROFILES_KEY = 'agentum_profiles';
const ACTIVE_KEY = 'agentum_active';
const LEGACY_TOKEN_KEY = 'agentum_token';

/** Profile that mirrors the original "talk to current origin" behaviour. */
const DEFAULT_PROFILE: Profile = {
  id: 'local',
  label: 'this server',
  baseUrl: '',
  token: ''
};

function safeStorage(): Storage | null {
  try {
    if (typeof localStorage === 'undefined') return null;
    return localStorage;
  } catch {
    return null;
  }
}

function loadAll(): Profile[] {
  const ls = safeStorage();
  if (!ls) return [DEFAULT_PROFILE];

  let parsed: Profile[] = [];
  try {
    const raw = ls.getItem(PROFILES_KEY);
    if (raw) {
      const arr = JSON.parse(raw);
      if (Array.isArray(arr)) {
        parsed = arr.filter(
          (p): p is Profile =>
            !!p &&
            typeof p.id === 'string' &&
            typeof p.label === 'string' &&
            typeof p.baseUrl === 'string' &&
            typeof p.token === 'string'
        );
      }
    }
  } catch {
    parsed = [];
  }

  if (parsed.length === 0) {
    // First load on a clean install OR a corrupted store. Seed with
    // the default profile and migrate the legacy single-token slot
    // into it so the user doesn't get logged out by this upgrade.
    const legacyToken = ls.getItem(LEGACY_TOKEN_KEY) ?? '';
    const seed: Profile = { ...DEFAULT_PROFILE, token: legacyToken };
    ls.setItem(PROFILES_KEY, JSON.stringify([seed]));
    return [seed];
  }
  return parsed;
}

function persistAll(list: Profile[]): void {
  const ls = safeStorage();
  if (!ls) return;
  ls.setItem(PROFILES_KEY, JSON.stringify(list));
}

function loadActiveId(): string {
  const ls = safeStorage();
  if (!ls) return DEFAULT_PROFILE.id;
  return ls.getItem(ACTIVE_KEY) ?? DEFAULT_PROFILE.id;
}

function persistActiveId(id: string): void {
  const ls = safeStorage();
  if (!ls) return;
  ls.setItem(ACTIVE_KEY, id);
}

// ---------- live store ----------

export const profiles = writable<Profile[]>(loadAll());
export const activeProfileId = writable<string>(loadActiveId());

profiles.subscribe(persistAll);
activeProfileId.subscribe(persistActiveId);

/** Synchronous accessor used by the api.ts request layer. */
export function getActiveProfile(): Profile {
  const list = get(profiles);
  const id = get(activeProfileId);
  return list.find((p) => p.id === id) ?? list[0] ?? DEFAULT_PROFILE;
}

/** Update the bearer token for the active profile (e.g. after login). */
export function setActiveToken(token: string): void {
  const id = get(activeProfileId);
  profiles.update((list) =>
    list.map((p) => (p.id === id ? { ...p, token } : p))
  );
  // Keep the legacy slot in sync so any code path that still reads it
  // sees a coherent value during the transition.
  const ls = safeStorage();
  if (ls) {
    if (token) ls.setItem(LEGACY_TOKEN_KEY, token);
    else ls.removeItem(LEGACY_TOKEN_KEY);
  }
}

export function clearActiveToken(): void {
  setActiveToken('');
}

export function setActiveProfile(id: string): void {
  const list = get(profiles);
  if (!list.some((p) => p.id === id)) return;
  activeProfileId.set(id);
}

export function upsertProfile(p: Profile): void {
  if (!isValidId(p.id)) {
    throw new Error('profile id must be alphanumeric (with - . _)');
  }
  profiles.update((list) => {
    const idx = list.findIndex((x) => x.id === p.id);
    if (idx >= 0) {
      const next = list.slice();
      next[idx] = p;
      return next;
    }
    return [...list, p];
  });
}

export function removeProfile(id: string): void {
  profiles.update((list) => {
    const next = list.filter((p) => p.id !== id);
    // Don't allow removing the last profile — leave the local default.
    if (next.length === 0) return [DEFAULT_PROFILE];
    return next;
  });
  if (get(activeProfileId) === id) {
    activeProfileId.set(get(profiles)[0]?.id ?? DEFAULT_PROFILE.id);
  }
}

function isValidId(id: string): boolean {
  return /^[a-zA-Z0-9._-]+$/.test(id);
}

/**
 * Resolve `path` (e.g. `/api/sessions`) against the active profile's
 * base URL. Returns `path` unchanged when the profile uses the current
 * origin so existing relative-fetch behaviour is preserved.
 */
export function apiUrl(path: string): string {
  const base = getActiveProfile().baseUrl;
  if (!base) return path;
  // Trim trailing slashes off the base; `path` is always a leading slash.
  return base.replace(/\/+$/, '') + path;
}

/**
 * Build a `ws(s)://…?token=…` URL against the active profile. Used for
 * `/api/events` and `/api/sessions/{id}/stream`. Falls back to the
 * page's own origin when the profile is empty-string-base.
 */
export function wsUrl(path: string): string {
  return wsUrlForProfile(getActiveProfile().id, path);
}

/**
 * Look up a profile by id. Returns the active profile when `id` is
 * empty / unknown so per-profile call sites degrade gracefully into
 * the active-profile path.
 */
export function profileById(id: string | null | undefined): Profile {
  if (!id) return getActiveProfile();
  const list = get(profiles);
  return list.find((p) => p.id === id) ?? getActiveProfile();
}

/**
 * Build an HTTP URL against `profileId`. Multi-endpoint aggregation
 * uses this to fan out fetches across every configured profile.
 * Falls back to the active profile when `profileId` doesn't match
 * any configured one (degrades to the existing single-endpoint path).
 */
export function apiUrlForProfile(profileId: string, path: string): string {
  const profile = profileById(profileId);
  const base = profile.baseUrl;
  if (!base) return path;
  return base.replace(/\/+$/, '') + path;
}

/**
 * Build a WS URL against `profileId`. Same fallback semantics as
 * `apiUrlForProfile`. The bearer token gets appended as `?token=…`
 * because browsers can't set custom headers on WS upgrades.
 */
export function wsUrlForProfile(profileId: string, path: string): string {
  const profile = profileById(profileId);
  const token = profile.token;
  let baseProto: string;
  let baseHost: string;
  if (profile.baseUrl) {
    try {
      const u = new URL(profile.baseUrl);
      baseProto = u.protocol === 'https:' ? 'wss:' : 'ws:';
      baseHost = u.host;
    } catch {
      baseProto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      baseHost = location.host;
    }
  } else {
    baseProto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    baseHost = location.host;
  }
  const url = `${baseProto}//${baseHost}${path}`;
  return token ? `${url}?token=${encodeURIComponent(token)}` : url;
}

/**
 * Profile-pinned `fetch`. Used by aggregating call sites (sessions
 * store, fleet view) to talk to a specific endpoint regardless of
 * which one is "active". Wraps the same auth-header injection the
 * single-profile `request` does in `api.ts`.
 */
export async function fetchProfile(
  profileId: string,
  path: string,
  init: RequestInit = {}
): Promise<Response> {
  const profile = profileById(profileId);
  const headers = new Headers(init.headers);
  if (!headers.has('content-type') && init.body) {
    headers.set('content-type', 'application/json');
  }
  // Loopback profile (baseUrl === '') talks to page origin — same server
  // as whatever the user is authenticated against. Falling back to the
  // active profile's token here fixes the silent 401 that hid local
  // sessions when the legacy migration didn't stamp the local row
  // (e.g. user logged in *after* adding a remote profile, so only the
  // active profile carries a token).
  let token = profile.token;
  if (!token && !profile.baseUrl) {
    const active = getActiveProfile();
    if (active.token) token = active.token;
  }
  if (token) headers.set('authorization', `Bearer ${token}`);
  return fetch(apiUrlForProfile(profileId, path), { ...init, headers });
}
