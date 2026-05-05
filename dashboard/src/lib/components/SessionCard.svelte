<script lang="ts">
  import { api, type Session } from '$lib/api';
  import StatusPill from './StatusPill.svelte';

  interface Props {
    session: Session;
    onChanged?: () => void;
  }
  let { session, onChanged }: Props = $props();

  let busy = $state<null | 'start' | 'stop' | 'kill' | 'delete'>(null);
  let error = $state<string | null>(null);

  function shortenPath(p: string, max = 36): string {
    if (p.length <= max) return p;
    return '…' + p.slice(p.length - max + 1);
  }

  function rel(ts: string | null): string {
    if (!ts) return '—';
    const d = new Date(ts);
    const diff = (Date.now() - d.getTime()) / 1000;
    if (diff < 60) return `${Math.floor(diff)}s ago`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return d.toLocaleDateString();
  }

  async function run<T>(label: typeof busy, fn: () => Promise<T>) {
    busy = label;
    error = null;
    try {
      await fn();
      onChanged?.();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = null;
    }
  }

  const YOLO_FLAG = '--dangerously-skip-permissions';
  const isYolo = $derived(session.flags.includes(YOLO_FLAG));

  function start(e: MouseEvent) { e.preventDefault(); run('start', () => api.startSession(session.id)); }
  function stop(e: MouseEvent)  { e.preventDefault(); run('stop',  () => api.stopSession(session.id)); }
  function kill(e: MouseEvent)  { e.preventDefault(); run('kill',  () => api.killSession(session.id)); }

  function del(e: MouseEvent) {
    e.preventDefault();
    const force = session.status === 'running';
    const verb = force ? 'kill and remove' : 'remove';
    if (!confirm(`${verb} session "${session.name}"?`)) return;
    run('delete', () => api.deleteSession(session.id, force));
  }
</script>

<article class="card surface" data-status={session.status}>
  <a class="link" href={`/sessions/${session.id}`}>
    <div class="row">
      <span class="eyebrow">Session</span>
      <div class="badges">
        {#if isYolo}
          <span class="yolo-badge" title="YOLO mode — permissions auto-approved">YOLO</span>
        {/if}
        <StatusPill status={session.status} />
      </div>
    </div>

    <h3 class="name">{session.name}</h3>

    <div class="meta">
      <span class="tool mono">{session.tool}</span>
      <span class="sep" aria-hidden="true">·</span>
      <span class="workdir mono" title={session.workdir}>{shortenPath(session.workdir)}</span>
    </div>

    <div class="footer">
      <span class="muted">last activity</span>
      <span class="rel mono">{rel(session.last_activity_at ?? session.updated_at)}</span>
    </div>
  </a>

  <div class="actions">
    {#if session.status === 'running'}
      <button class="btn-subtle" type="button" onclick={stop} disabled={busy !== null} title="Graceful stop (CLI: agentum down)">
        {busy === 'stop' ? '…' : 'stop'}
      </button>
      <button class="btn-subtle" type="button" onclick={kill} disabled={busy !== null} title="Force kill (CLI: agentum kill)">
        {busy === 'kill' ? '…' : 'kill'}
      </button>
    {:else}
      <button class="btn-subtle" type="button" onclick={start} disabled={busy !== null} title="Start session (CLI: agentum up)">
        {busy === 'start' ? '…' : 'start'}
      </button>
    {/if}
    <button class="btn-subtle danger" type="button" onclick={del} disabled={busy !== null} title="Remove (CLI: agentum rm)">
      {busy === 'delete' ? '…' : 'rm'}
    </button>
  </div>

  {#if error}
    <div class="error mono" title={error}>{error}</div>
  {/if}
</article>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px;
    color: var(--text);
    transition: border-color 120ms ease, background 120ms ease;
    position: relative;
    overflow: hidden;
  }
  .card::after {
    content: "";
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 2px;
    background: transparent;
    transition: background 120ms ease;
  }
  .card:hover { border-color: var(--accent); }
  .card[data-status="running"]::after { background: var(--success); }
  .card[data-status="crashed"]::after { background: var(--danger); }

  .link {
    display: flex;
    flex-direction: column;
    gap: 12px;
    color: var(--text);
    text-decoration: none;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .badges {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .yolo-badge {
    font-family: var(--font-mono);
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    padding: 1px 6px;
    border-radius: 4px;
    color: var(--bg);
    background: var(--warning, #e6a817);
  }
  .name {
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.02em;
    margin: 0;
    line-height: 1.15;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-2);
    font-size: 12.5px;
  }
  .tool {
    color: var(--cta);
    font-size: 12px;
    letter-spacing: 0.02em;
  }
  .sep { color: var(--muted); }
  .workdir {
    color: var(--text-2);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    color: var(--muted);
    font-size: 11.5px;
    padding-top: 10px;
    border-top: 1px solid var(--border-2);
  }
  .muted { color: var(--muted); text-transform: lowercase; }
  .rel { color: var(--text-2); font-size: 11.5px; }

  .actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .actions .btn-subtle {
    flex: 1;
    min-width: 56px;
  }

  .error {
    font-size: 11px;
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    padding: 6px 10px;
    border-radius: var(--radius-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
