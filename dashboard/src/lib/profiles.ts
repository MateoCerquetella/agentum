/**
 * Named-endpoint profiles for the dashboard.
 *
 * Source of truth lives on the page-origin daemon at
 * `/api/profiles` — the same `~/.config/agentum/profiles.toml` the
 * TUI reads. Adding a profile in the TUI (`agentum profiles add …`)
 * shows up here on the next load and vice-versa.
 *
 * Browser-local state stays local:
 *   - **tokens** are bearer credentials earned per-endpoint at login
 *     and never leave this device.
 *   - **labels** are pure UX — the daemon stores names; labels are
 *     looked up locally by name.
 *   - the **active profile id** is a per-tab UI preference.
 *
 * Storage keys:
 *   - `agentum_profile_tokens` → `{ [id]: token }`
 *   - `agentum_profile_labels` → `{ [id]: label }`
 *   - `agentum_profile_cache`  → cached server response for fast first paint
 *   - `agentum_active`         → string id, the active profile
 *
 * A **synthetic** profile with id `same-origin` and empty `baseUrl`
 * is always present at the head of the list and represents "the
 * daemon that served this page." It's never sent to `/api/profiles` —
 * the server has its own loopback profile if it wants one.
 *
 * Legacy single-token (`agentum_token`) and pre-API multi-profile
 * (`agentum_profiles`) localStorage shapes are migrated on first
 * load.
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
   * as this page" (only valid for the synthetic profile).
   */
  baseUrl: string;
  /** Bearer token earned for this endpoint. Empty until the user logs in. */
  token: string;
}

/** Server-side wire shape — mirrors `agentum_core::profiles::Profile`. */
interface ServerProfile {
  url: string;
  fingerprint?: string | null;
  insecure?: boolean;
}

interface ServerProfilesFile {
  default: string | null;
  profiles: Record<string, ServerProfile>;
}

const TOKENS_KEY = 'agentum_profile_tokens';
const LABELS_KEY = 'agentum_profile_labels';
const CACHE_KEY = 'agentum_profile_cache';
const ACTIVE_KEY = 'agentum_active';

// Legacy keys, read once during migration.
const LEGACY_PROFILES_KEY = 'agentum_profiles';
const LEGACY_TOKEN_KEY = 'agentum_token';
const LEGACY_MIGRATED_KEY = 'agentum_profiles_migrated';

const SYNTHETIC_ID = 'same-origin';
const SYNTHETIC_LABEL = 'this server';

const SYNTHETIC_BASE: Profile = {
  id: SYNTHETIC_ID,
  label: SYNTHETIC_LABEL,
  baseUrl: '',
  token: ''
};

// ---------- localStorage primitives ----------

function safeStorage(): Storage | null {
  try {
    if (typeof localStorage === 'undefined') return null;
    return localStorage;
  } catch {
    return null;
  }
}

function readJSON<T>(key: string, fallback: T): T {
  const ls = safeStorage();
  if (!ls) return fallback;
  try {
    const raw = ls.getItem(key);
    if (!raw) return fallback;
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

function writeJSON<T>(key: string, value: T): void {
  const ls = safeStorage();
  if (!ls) return;
  ls.setItem(key, JSON.stringify(value));
}

function loadTokens(): Record<string, string> {
  return readJSON<Record<string, string>>(TOKENS_KEY, {});
}

function loadLabels(): Record<string, string> {
  return readJSON<Record<string, string>>(LABELS_KEY, {});
}

function loadCache(): Record<string, ServerProfile> {
  return readJSON<Record<string, ServerProfile>>(CACHE_KEY, {});
}

function loadActiveIdRaw(): string {
  const ls = safeStorage();
  return ls?.getItem(ACTIVE_KEY) ?? SYNTHETIC_ID;
}

function saveActiveIdRaw(id: string): void {
  const ls = safeStorage();
  ls?.setItem(ACTIVE_KEY, id);
}

// ---------- legacy migration ----------

/**
 * Promote the pre-API `agentum_profiles` array (and the even older
 * `agentum_token` single-slot) into the new split storage shape.
 *
 * Token + label promotion is unconditional — these stay browser-local
 * and don't require the daemon to be reachable. Profile POSTs to the
 * daemon are returned for the caller to fire; the migration sentinel
 * is only set once at least one POST lands so an unauthenticated
 * first-load retries after the user logs in.
 *
 * Returns a list of `(name, ServerProfile)` to push to the daemon.
 */
function migrateLegacy(): { name: string; profile: ServerProfile }[] {
  const ls = safeStorage();
  if (!ls) return [];
  if (ls.getItem(LEGACY_MIGRATED_KEY)) return [];

  const tokens = loadTokens();
  const labels = loadLabels();
  const pushes: { name: string; profile: ServerProfile }[] = [];

  // Old single-token slot. If we have it but no per-profile token
  // for the synthetic, copy it over.
  const legacyToken = ls.getItem(LEGACY_TOKEN_KEY);
  if (legacyToken && !tokens[SYNTHETIC_ID]) {
    tokens[SYNTHETIC_ID] = legacyToken;
  }

  const legacyArr = readJSON<unknown>(LEGACY_PROFILES_KEY, null);
  if (Array.isArray(legacyArr)) {
    for (const raw of legacyArr) {
      if (!raw || typeof raw !== 'object') continue;
      const p = raw as Partial<Profile>;
      if (typeof p.id !== 'string' || typeof p.baseUrl !== 'string') continue;

      // Capture token & label into the new stores, keyed by id.
      if (typeof p.token === 'string' && p.token) tokens[p.id] = p.token;
      if (typeof p.label === 'string') labels[p.id] = p.label;

      // Old "local" with empty baseUrl is the synthetic — keep its
      // token/label, don't sync to the daemon.
      if (p.baseUrl === '') {
        if (typeof p.token === 'string' && p.token) tokens[SYNTHETIC_ID] = p.token;
        if (p.id !== SYNTHETIC_ID && typeof p.label === 'string') {
          labels[SYNTHETIC_ID] = p.label;
        }
        continue;
      }

      // Real remote — schedule a POST after boot.
      pushes.push({
        name: p.id,
        profile: { url: p.baseUrl }
      });
    }
  }

  writeJSON(TOKENS_KEY, tokens);
  writeJSON(LABELS_KEY, labels);
  // Note: we do NOT set LEGACY_MIGRATED_KEY here. The caller sets it
  // only after the POSTs land (or after there's nothing left to push)
  // so an unauthenticated boot retries the migration on the next load.
  // The legacy `agentum_profiles` / `agentum_token` keys stay in place
  // — leaving them lets a downgrade fall back gracefully.
  return pushes;
}

// ---------- merge logic ----------

function syntheticProfile(): Profile {
  const tokens = loadTokens();
  const labels = loadLabels();
  return {
    ...SYNTHETIC_BASE,
    label: labels[SYNTHETIC_ID] ?? SYNTHETIC_LABEL,
    token: tokens[SYNTHETIC_ID] ?? ''
  };
}

function serverEntryToProfile(name: string, sp: ServerProfile): Profile {
  const tokens = loadTokens();
  const labels = loadLabels();
  return {
    id: name,
    label: labels[name] ?? name,
    baseUrl: sp.url,
    token: tokens[name] ?? ''
  };
}

function mergeFromCache(): Profile[] {
  const cache = loadCache();
  const remote = Object.entries(cache).map(([n, p]) => serverEntryToProfile(n, p));
  return [syntheticProfile(), ...remote];
}

// ---------- server I/O ----------

/**
 * GET /api/profiles against the *page origin* — the profile list is
 * owned by the daemon that served this SPA, not by whatever profile
 * happens to be active. The synthetic profile's token is used for
 * auth because that's the same-origin credential.
 */
async function fetchServerProfiles(): Promise<ServerProfilesFile | null> {
  const tokens = loadTokens();
  const headers: Record<string, string> = {};
  const token = tokens[SYNTHETIC_ID];
  if (token) headers['authorization'] = `Bearer ${token}`;
  try {
    const res = await fetch('/api/profiles', { headers });
    if (!res.ok) return null;
    return (await res.json()) as ServerProfilesFile;
  } catch {
    return null;
  }
}

/**
 * Mutating calls use `keepalive: true` so a `location.reload()`
 * immediately after `upsertProfile`/`removeProfile` (TokenGate's
 * setup flow) doesn't cancel the in-flight write. Without this the
 * daemon never sees the POST and the next refresh wipes the
 * optimistically-cached entry.
 */
async function postServerProfile(name: string, profile: ServerProfile): Promise<boolean> {
  const tokens = loadTokens();
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  const token = tokens[SYNTHETIC_ID];
  if (token) headers['authorization'] = `Bearer ${token}`;
  try {
    const res = await fetch('/api/profiles', {
      method: 'POST',
      headers,
      body: JSON.stringify({ name, ...profile }),
      keepalive: true
    });
    // 409 = already exists. Not an error from the caller's perspective
    // for the migration path; it means the daemon already has this
    // profile under the same name.
    return res.ok || res.status === 409;
  } catch {
    return false;
  }
}

async function putServerProfile(name: string, profile: ServerProfile): Promise<boolean> {
  const tokens = loadTokens();
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  const token = tokens[SYNTHETIC_ID];
  if (token) headers['authorization'] = `Bearer ${token}`;
  try {
    const res = await fetch(`/api/profiles/${encodeURIComponent(name)}`, {
      method: 'PUT',
      headers,
      body: JSON.stringify(profile),
      keepalive: true
    });
    return res.ok;
  } catch {
    return false;
  }
}

async function deleteServerProfile(name: string): Promise<boolean> {
  const tokens = loadTokens();
  const headers: Record<string, string> = {};
  const token = tokens[SYNTHETIC_ID];
  if (token) headers['authorization'] = `Bearer ${token}`;
  try {
    const res = await fetch(`/api/profiles/${encodeURIComponent(name)}`, {
      method: 'DELETE',
      headers,
      keepalive: true
    });
    // 404 on delete is benign — somebody else already removed it.
    return res.ok || res.status === 404;
  } catch {
    return false;
  }
}

// ---------- live stores ----------

export const profiles = writable<Profile[]>(mergeFromCache());
export const activeProfileId = writable<string>(loadActiveIdRaw());

activeProfileId.subscribe(saveActiveIdRaw);

// ---------- bootstrap ----------

/**
 * Hydrate the store from the daemon. Idempotent — call as often as
 * you like. Quiet on failure: the cache-backed render keeps working
 * when the daemon is down.
 */
export async function refreshProfiles(): Promise<void> {
  const file = await fetchServerProfiles();
  if (!file) return;
  // Cache for fast first paint next reload.
  writeJSON(CACHE_KEY, file.profiles);
  profiles.set([
    syntheticProfile(),
    ...Object.entries(file.profiles).map(([n, p]) => serverEntryToProfile(n, p))
  ]);
}

/**
 * Best-effort migration of legacy localStorage profiles. Sets the
 * `LEGACY_MIGRATED_KEY` sentinel only after every POST succeeds (or
 * there's nothing to push) so an unauthenticated first boot retries
 * the migration after the user logs in.
 */
async function pushMigratedProfiles(): Promise<void> {
  const pending = migrateLegacy();
  if (pending.length === 0) {
    safeStorage()?.setItem(LEGACY_MIGRATED_KEY, '1');
    return;
  }
  const results = await Promise.all(
    pending.map(({ name, profile }) => postServerProfile(name, profile))
  );
  if (results.every((ok) => ok)) {
    safeStorage()?.setItem(LEGACY_MIGRATED_KEY, '1');
  }
}

// Kick off bootstrap on module load. We don't await — the cache-backed
// list is already in the store, so renders work immediately. The
// reconciliation happens shortly after.
if (typeof window !== 'undefined') {
  void (async () => {
    await pushMigratedProfiles();
    await refreshProfiles();
  })();
}

// ---------- public mutations ----------

export function getActiveProfile(): Profile {
  const list = get(profiles);
  const id = get(activeProfileId);
  return list.find((p) => p.id === id) ?? list[0] ?? SYNTHETIC_BASE;
}

/** Update the bearer token for the active profile (e.g. after login). */
export function setActiveToken(token: string): void {
  const id = get(activeProfileId);
  const tokens = loadTokens();
  if (token) tokens[id] = token;
  else delete tokens[id];
  writeJSON(TOKENS_KEY, tokens);

  profiles.update((list) => list.map((p) => (p.id === id ? { ...p, token } : p)));

  // Keep the legacy single-slot in sync for any code path still reading
  // it directly during the transition. Cleaned up once nothing reads it.
  const ls = safeStorage();
  if (ls) {
    if (token) ls.setItem(LEGACY_TOKEN_KEY, token);
    else ls.removeItem(LEGACY_TOKEN_KEY);
  }

  // A fresh same-origin token unblocks the legacy migration if it
  // failed earlier due to a 401 (auth-gated daemon, user hadn't
  // logged in yet). Re-run; the sentinel keeps it a no-op once
  // everything has been pushed.
  if (token && id === SYNTHETIC_ID) {
    void (async () => {
      await pushMigratedProfiles();
      await refreshProfiles();
    })();
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

/**
 * Add or update a profile. Optimistically updates the local store,
 * fires the matching server call, and rolls back on failure.
 *
 * The synthetic same-origin profile is never persisted to the daemon —
 * label/token changes against it stay local.
 */
export function upsertProfile(p: Profile): void {
  if (!isValidId(p.id)) {
    throw new Error('profile id must be alphanumeric (with - . _)');
  }

  // Stash label + token in their respective local stores. These never
  // round-trip through the daemon.
  const labels = loadLabels();
  labels[p.id] = p.label;
  writeJSON(LABELS_KEY, labels);
  if (p.token) {
    const tokens = loadTokens();
    tokens[p.id] = p.token;
    writeJSON(TOKENS_KEY, tokens);
  }

  // Optimistic store update.
  const prev = get(profiles);
  profiles.update((list) => {
    const idx = list.findIndex((x) => x.id === p.id);
    if (idx >= 0) {
      const next = list.slice();
      next[idx] = p;
      return next;
    }
    return [...list, p];
  });

  // Synthetic profile stops here — we don't sync "same-origin" upstream.
  if (p.id === SYNTHETIC_ID) return;

  const wire: ServerProfile = { url: p.baseUrl };
  const isUpdate = prev.some((x) => x.id === p.id);

  // Eagerly mirror to the cache so a `location.reload()` immediately
  // after `upsertProfile` (TokenGate's setup flow) still renders the
  // new entry on first paint. The next `refreshProfiles` call will
  // either confirm it (POST landed → daemon returns it) or wipe it
  // (POST failed → daemon's list overrules). Combined with
  // `keepalive: true` on the POST, the daemon almost always wins
  // the race in practice.
  const prevCache = loadCache();
  const nextCache = { ...prevCache, [p.id]: wire };
  writeJSON(CACHE_KEY, nextCache);

  void (async () => {
    const ok = isUpdate
      ? await putServerProfile(p.id, wire)
      : await postServerProfile(p.id, wire);
    if (!ok) {
      // Roll both the store and the cache back. The server's view wins.
      profiles.set(prev);
      writeJSON(CACHE_KEY, prevCache);
    }
  })();
}

export function removeProfile(id: string): void {
  // Refuse to remove the synthetic profile — it represents the page
  // origin and there's no meaningful "delete" for that.
  if (id === SYNTHETIC_ID) return;

  const prev = get(profiles);
  profiles.update((list) => list.filter((p) => p.id !== id));

  // If we just removed the active profile, fall back to the head.
  if (get(activeProfileId) === id) {
    const fallback = get(profiles)[0];
    activeProfileId.set(fallback?.id ?? SYNTHETIC_ID);
  }

  // Drop the local token + label cache for the deleted id.
  const tokens = loadTokens();
  delete tokens[id];
  writeJSON(TOKENS_KEY, tokens);
  const labels = loadLabels();
  delete labels[id];
  writeJSON(LABELS_KEY, labels);

  // Eager cache drop, mirroring the eager add in `upsertProfile`.
  const prevCache = loadCache();
  const nextCache = { ...prevCache };
  delete nextCache[id];
  writeJSON(CACHE_KEY, nextCache);

  void (async () => {
    const ok = await deleteServerProfile(id);
    if (!ok) {
      profiles.set(prev);
      writeJSON(CACHE_KEY, prevCache);
    }
  })();
}

function isValidId(id: string): boolean {
  return /^[a-zA-Z0-9._-]+$/.test(id);
}

// ---------- URL builders (unchanged) ----------

/**
 * Resolve `path` (e.g. `/api/sessions`) against the active profile's
 * base URL. Returns `path` unchanged when the profile uses the current
 * origin so existing relative-fetch behaviour is preserved.
 */
export function apiUrl(path: string): string {
  const base = getActiveProfile().baseUrl;
  if (!base) return path;
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
