import { writable, type Writable } from 'svelte/store';
import { onEvent } from './events';

export interface HostMetrics {
  cpu_pct: number;
  cores: number[];
  mem_used: number;
  mem_total: number;
  swap_used: number;
  swap_total: number;
  cpu_count: number;
}

export interface HostHistory {
  cpu: number[];
  memPct: number[];
  latest: HostMetrics | null;
}

const HISTORY_LEN = 60;

const initial: HostHistory = { cpu: [], memPct: [], latest: null };

export const host: Writable<HostHistory> = writable(initial);

let unsubscribed = false;
let stopBus: (() => void) | null = null;

/**
 * Boot the host-metrics feed once per page load. Pulls a one-shot
 * `/api/host/metrics` for the initial point so the sparkline isn't
 * empty pre-WS, then attaches to the bus to receive `host.metrics`
 * ticks every couple of seconds. Calling more than once is a no-op
 * — we keep a single subscription and history rolling for the life
 * of the tab.
 */
export function startHostMetrics(): void {
  if (stopBus || unsubscribed) return;

  // One-shot prime. Doesn't block the WS attach below.
  fetch('/api/host/metrics', { headers: authHeader() })
    .then((r) => (r.ok ? (r.json() as Promise<HostMetrics>) : null))
    .then((m) => {
      if (!m) return;
      pushSample(m);
    })
    .catch(() => {});

  stopBus = onEvent((ev) => {
    if (ev.kind !== 'host.metrics') return;
    const m = ev.payload as unknown as HostMetrics;
    if (!m || typeof m.cpu_pct !== 'number') return;
    pushSample(m);
  });
}

export function stopHostMetrics(): void {
  unsubscribed = true;
  stopBus?.();
  stopBus = null;
}

function pushSample(m: HostMetrics) {
  const memPct = m.mem_total > 0 ? (m.mem_used / m.mem_total) * 100 : 0;
  host.update((h) => ({
    latest: m,
    cpu: clamp([...h.cpu, m.cpu_pct]),
    memPct: clamp([...h.memPct, memPct])
  }));
}

function clamp(xs: number[]): number[] {
  return xs.length > HISTORY_LEN ? xs.slice(xs.length - HISTORY_LEN) : xs;
}

function authHeader(): HeadersInit {
  const t = typeof localStorage !== 'undefined' ? localStorage.getItem('agentum_token') : null;
  return t ? { Authorization: `Bearer ${t}` } : {};
}

export function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}
