import { writable } from 'svelte/store';
import { setToken, probeAuth } from '$lib/api';

export type AuthState = 'unknown' | 'ok' | 'needs-token' | 'unreachable';

export const authState = writable<AuthState>('unknown');

export async function refreshAuth() {
  const result = await probeAuth();
  if (result === 'ok') authState.set('ok');
  else if (result === 'unauthorized') authState.set('needs-token');
  else authState.set('unreachable');
}

export async function setTokenAndRetry(token: string) {
  setToken(token.trim());
  await refreshAuth();
}
