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

  function start(e: MouseEvent)  { e.preventDefault(); run('start',  () => api.startSession(session.id)); }
  function stop(e: MouseEvent)   { e.preventDefault(); run('stop',   () => api.stopSession(session.id)); }
  function kill(e: MouseEvent)   { e.preventDefault(); run('kill',   () => api.killSession(session.id)); }

  function del(e: MouseEvent) {
    e.preventDefault();
    const force = session.status === 'running';
    const verb = force ? 'kill and remove' : 'remove';
    if (!confirm(`${verb} session "${session.name}"?`)) return;
    run('delete', () => api.deleteSession(session.id, force));
  }
</script>

<div class="card" data-status={session.status}>
  <a class="link" href={`/sessions/${session.id}`}>
    <header>
      <div class="name">{session.name}</div>
      <StatusPill status={session.status} />
    </header>

    <div class="meta">
      <span class="tool" title="tool">{session.tool}</span>
      <span class="dot" aria-hidden="true">·</span>
      <span class="workdir mono" title={session.workdir}>{shortenPath(session.workdir)}</span>
    </div>

    <footer>
      <span class="muted">last activity</span>
      <span>{rel(session.last_activity_at ?? session.updated_at)}</span>
    </footer>
  </a>

  <div class="actions">
    {#if session.status === 'running'}
      <button type="button" onclick={stop} disabled={busy !== null} title="Graceful stop (CLI: agentum down)">
        {busy === 'stop' ? '…' : 'stop'}
      </button>
      <button type="button" onclick={kill} disabled={busy !== null} title="Force kill (CLI: agentum kill)">
        {busy === 'kill' ? '…' : 'kill'}
      </button>
    {:else}
      <button type="button" onclick={start} disabled={busy !== null} title="Start session (CLI: agentum up)">
        {busy === 'start' ? '…' : 'start'}
      </button>
    {/if}
    <button type="button" class="danger" onclick={del} disabled={busy !== null} title="Remove (CLI: agentum rm)">
      {busy === 'delete' ? '…' : 'rm'}
    </button>
  </div>

  {#if error}
    <div class="error" title={error}>{error}</div>
  {/if}
</div>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 1rem 1.1rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    color: var(--text);
    transition: border-color 120ms ease, background 120ms ease;
  }
  .card:hover {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .link {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    color: var(--text);
    text-decoration: none;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .name {
    font-family: var(--font-display);
    font-size: 1.05rem;
    font-weight: 600;
    letter-spacing: -0.005em;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--text-2);
    font-size: 0.85rem;
  }
  .tool { color: var(--accent); }
  .workdir { font-size: 0.8rem; }
  footer {
    display: flex;
    justify-content: space-between;
    color: var(--muted);
    font-size: 0.78rem;
    padding-top: 0.4rem;
    border-top: 1px solid var(--border);
  }
  .muted { color: var(--muted); }
  .mono { font-family: var(--font-mono); }

  .actions {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
  }
  .actions button {
    flex: 1;
    min-width: 3.2rem;
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg);
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.75rem;
    cursor: pointer;
  }
  .actions button:hover:not(:disabled) {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions .danger:hover:not(:disabled) {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 40%, var(--border));
  }

  .error {
    font-family: var(--font-mono);
    font-size: 0.72rem;
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    padding: 0.3rem 0.5rem;
    border-radius: 4px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
