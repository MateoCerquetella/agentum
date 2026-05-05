import { writable } from 'svelte/store';
import { api, setToken, ApiError } from '$lib/api';

export type AuthState =
  | 'unknown'
  | 'ok'
  | 'needs-login'
  | 'needs-setup'
  | 'unreachable';

export const authState = writable<AuthState>('unknown');
export const currentUser = writable<string | null>(null);

/**
 * Decide the gate state by combining `/api/auth/status` (server-side fact:
 * has anyone registered yet?) with `/api/auth/me` (does our cached token
 * still validate?).
 */
export async function refreshAuth() {
  let status;
  try {
    status = await api.authStatus();
  } catch {
    authState.set('unreachable');
    return;
  }

  if (status.needs_setup) {
    authState.set('needs-setup');
    currentUser.set(null);
    return;
  }

  // Users exist; validate any cached token.
  try {
    const me = await api.me();
    currentUser.set(me.username);
    authState.set('ok');
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      currentUser.set(null);
      authState.set('needs-login');
    } else {
      authState.set('unreachable');
    }
  }
}

export async function login(username: string, password: string) {
  const r = await api.login(username, password);
  setToken(r.token);
  currentUser.set(r.username);
  authState.set('ok');
}

export async function register(
  username: string,
  password: string,
  pin?: string
) {
  const r = await api.register(username, password, pin);
  setToken(r.token);
  currentUser.set(r.username);
  authState.set('ok');
}

export async function logout() {
  try {
    await api.logout();
  } catch {
    // best-effort; we're dropping the token regardless
  }
  setToken(null);
  currentUser.set(null);
  authState.set('needs-login');
}
