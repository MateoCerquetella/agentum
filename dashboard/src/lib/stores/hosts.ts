import { writable, get } from 'svelte/store';
import { api, type Host, type NewHost } from '$lib/api';
import { activeProfileId } from '$lib/profiles';

export const hosts = writable<Host[]>([]);
export const hostsLoading = writable(false);
export const hostsError = writable<string | null>(null);

let started = false;
let unsubProfile: (() => void) | null = null;
let refreshSeq = 0;

export function hostLabel(h: Host): string {
  if (h.kind === 'local') return h.name || 'local';
  return `${h.name} · ${h.user}@${h.hostname}:${h.port}`;
}

export async function refreshHosts(): Promise<void> {
  const seq = ++refreshSeq;
  const profileId = get(activeProfileId);
  hostsLoading.set(true);
  hostsError.set(null);
  try {
    const list = await api.listHostsOn(profileId);
    if (seq !== refreshSeq || profileId !== get(activeProfileId)) return;
    hosts.set(list);
  } catch (e) {
    if (seq !== refreshSeq || profileId !== get(activeProfileId)) return;
    hosts.set([]);
    hostsError.set(e instanceof Error ? e.message : String(e));
  } finally {
    if (seq === refreshSeq) hostsLoading.set(false);
  }
}

export function startHostsStore(): void {
  if (started) return;
  started = true;
  void refreshHosts();
  unsubProfile = activeProfileId.subscribe(() => {
    hosts.set([]);
    void refreshHosts();
  });
}

export function stopHostsStore(): void {
  unsubProfile?.();
  unsubProfile = null;
  started = false;
}

export async function addHost(input: NewHost): Promise<Host> {
  const created = await api.createHost(input);
  hosts.set([...get(hosts).filter((h) => h.id !== created.id), created]);
  return created;
}

export async function removeHost(id: string): Promise<void> {
  await api.deleteHost(id);
  hosts.set(get(hosts).filter((h) => h.id !== id));
}
