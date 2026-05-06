import type { Session, SessionState, Status } from '$lib/api';

/** Map server `Status` → design lifecycle `state`. Server-pushed
 *  `compact` overrides this when set on the Session. */
export function deriveState(s: Session): SessionState {
  if (s.state) return s.state;
  if (s.status === 'crashed') return 'crash';
  if (s.status === 'running') return 'live';
  return 'idle';
}

/** ctx defaults to 100 (full) when the backend hasn't populated it
 *  yet — that way ctx-driven affordances stay quiet pre-rollout. */
export function ctxOf(s: Session): number {
  return typeof s.ctx === 'number' ? s.ctx : 100;
}

/** "warm-zone" colors for the ctx pill. */
export function ctxColor(ctx: number): string {
  if (ctx >= 70) return 'var(--green)';
  if (ctx >= 50) return 'var(--amber)';
  return 'var(--cta)';
}

/** Compact: 128.4k / 3.2M. Falsey input → '—'. */
export function fmtTokens(n: number | null | undefined): string {
  if (n == null) return '—';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

export function fmtCost(usd: number | null | undefined): string {
  if (usd == null) return '—';
  if (usd >= 100) return `$${usd.toFixed(0)}`;
  return `$${usd.toFixed(2)}`;
}

export function fmtUptime(secs: number | null | undefined, fallbackFromCreated?: string): string {
  let s: number | null = null;
  if (typeof secs === 'number') s = secs;
  else if (fallbackFromCreated) {
    const ms = Date.now() - new Date(fallbackFromCreated).getTime();
    s = Math.max(0, Math.floor(ms / 1000));
  }
  if (s == null) return '—';
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}:${sec.toString().padStart(2, '0')}`;
  return `${m.toString().padStart(2, '0')}:${sec.toString().padStart(2, '0')}`;
}

export function fmtRel(ts: string | null | undefined): string {
  if (!ts) return '—';
  const d = new Date(ts);
  const diff = (Date.now() - d.getTime()) / 1000;
  if (diff < 60) return `${Math.floor(diff)}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

/** Tool short tag for the FleetRow / SessionRail. */
export function toolShort(tool: string | null): string {
  if (!tool) return '';
  if (tool === 'claude-code') return 'cc';
  return tool;
}

/** "Last activity" cell — server-pushed `last_log`, falling back to the
 *  most recent log line equivalent we can synthesize from existing
 *  session metadata. */
export function lastLogLine(s: Session): string {
  if (s.last_log && s.last_log.trim().length > 0) return s.last_log.trim();
  if (s.tmux_target) return s.tmux_target;
  return s.workdir;
}

/** "Good morning, X." — local-time aware. */
export function greeting(now = new Date()): string {
  const h = now.getHours();
  if (h < 5) return 'Up late';
  if (h < 12) return 'Good morning';
  if (h < 17) return 'Good afternoon';
  if (h < 21) return 'Good evening';
  return 'Good evening';
}

export function ctxFillClass(ctx: number): 'low' | 'mid' | '' {
  if (ctx < 50) return 'low';
  if (ctx < 70) return 'mid';
  return '';
}
