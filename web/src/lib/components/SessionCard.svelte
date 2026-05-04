<script lang="ts">
  import type { Session } from '$lib/api';
  import StatusPill from './StatusPill.svelte';

  interface Props { session: Session }
  let { session }: Props = $props();

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
</script>

<a class="card" href={`/sessions/${session.id}`} data-status={session.status}>
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
    transition: transform 80ms ease, border-color 120ms ease, background 120ms ease;
    text-decoration: none;
  }
  .card:hover {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
    transform: translateY(-1px);
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
</style>
