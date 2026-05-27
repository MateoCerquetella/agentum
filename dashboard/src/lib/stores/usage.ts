/**
 * Plan-usage poller — drives the sidebar footer chip.
 *
 * Pulls `/api/usage` on a slow timer (60s). Filesystem scans on the
 * backend are bounded by mtime + early-exit, so this is cheap enough
 * to keep running for the life of the tab without burning resources.
 *
 * The store stays compatible with multi-profile setups by re-fetching
 * whenever the active profile changes — usage is host-local data, so
 * a switch to a remote endpoint surfaces *that* host's plan headroom,
 * not the dashboard origin's.
 */

import { writable, derived, type Readable } from 'svelte/store';
import { api, type UsageBundle } from '$lib/api';
import { activeProfileId } from '$lib/profiles';

const POLL_MS = 60_000;

export const usage = writable<UsageBundle | null>(null);
let pollHandle: ReturnType<typeof setInterval> | null = null;
let inflight = false;

async function refresh(): Promise<void> {
  if (inflight) return;
  inflight = true;
  try {
    const snap = await api.getUsage();
    usage.set(snap);
  } catch {
    // Best-effort: an offline daemon shouldn't blank the chip mid-edit.
    // Keep the last good snapshot; next tick retries.
  } finally {
    inflight = false;
  }
}

let startedOnce = false;
let lastProfile: string | null = null;
let unsubProfile: (() => void) | null = null;

export function startUsagePoll(): void {
  if (startedOnce) return;
  startedOnce = true;
  void refresh();
  pollHandle = setInterval(() => void refresh(), POLL_MS);
  unsubProfile = activeProfileId.subscribe((id) => {
    if (id !== lastProfile) {
      lastProfile = id;
      usage.set(null);
      void refresh();
    }
  });
}

export function stopUsagePoll(): void {
  if (pollHandle) clearInterval(pollHandle);
  pollHandle = null;
  unsubProfile?.();
  unsubProfile = null;
  startedOnce = false;
}

/** Imperative refetch after the user triggers a connect-account flow. */
export function refreshUsageNow(): void {
  void refresh();
}

export interface UsageChip {
  show: boolean;
  label: string;
  /** 0–100 when known, null when only absolute tokens are available. */
  percent: number | null;
  detail: string;
  /** Right-side hint, e.g. "resets in 1h 23m". */
  resets: string | null;
  severity: 'ok' | 'warn' | 'danger';
}

function formatTokens(n: number): string {
  if (n < 1_000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

function fmtCountdown(target: number, now: number): string | null {
  const diff = target - now;
  if (diff <= 0) return 'resets now';
  const mins = Math.floor(diff / 60_000);
  if (mins < 60) return `resets in ${mins}m`;
  const hours = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem === 0 ? `resets in ${hours}h` : `resets in ${hours}h ${rem}m`;
}

export const claudeChip: Readable<UsageChip> = derived(usage, ($u): UsageChip => {
  if (!$u || !$u.claude.claude_installed) return blankChip();
  const c = $u.claude;
  const now = $u.generated_at_ms || Date.now();
  const resetMs = c.window_end_ms;
  const resets = resetMs ? fmtCountdown(resetMs, now) : null;
  const tokensLabel = formatTokens(c.window_tokens);
  return {
    show: true,
    label: `Claude · ${tokensLabel}`,
    percent: null,
    detail: `${c.window_tokens.toLocaleString()} tokens in last 5h window. All-time: ${c.all_time_tokens.toLocaleString()}.`,
    resets,
    severity: 'ok'
  };
});

export const codexChip: Readable<UsageChip> = derived(usage, ($u): UsageChip => {
  if (!$u || !$u.codex.codex_installed || !$u.codex.primary) return blankChip();
  const p = $u.codex.primary;
  const now = $u.generated_at_ms || Date.now();
  const resets = p.resets_at > 0 ? fmtCountdown(p.resets_at * 1000, now) : null;
  const plan = $u.codex.plan_type ? ` (${$u.codex.plan_type})` : '';
  const severity: UsageChip['severity'] =
    p.used_percent >= 95 ? 'danger' : p.used_percent >= 75 ? 'warn' : 'ok';
  return {
    show: true,
    label: `Codex · ${Math.round(p.used_percent)}%`,
    percent: p.used_percent,
    detail: `${p.used_percent.toFixed(1)}% of ${p.window_minutes / 60}h Codex window${plan}.`,
    resets,
    severity
  };
});

function blankChip(): UsageChip {
  const sev: UsageChip['severity'] = 'ok';
  return { show: false, label: '', percent: null, detail: '', resets: null, severity: sev };
}
