<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api, type Session } from '$lib/api';
  import StatusPill from '$components/StatusPill.svelte';

  let session = $state<Session | null>(null);
  let error = $state<string | null>(null);

  onMount(async () => {
    const id = page.params.id;
    if (!id) {
      error = 'missing session id';
      return;
    }
    try {
      session = await api.getSession(id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  });
</script>

<a class="back" href="/">← all sessions</a>

{#if error}
  <div class="error">Failed to load: <code>{error}</code></div>
{:else if !session}
  <div class="muted">loading…</div>
{:else}
  <header class="head">
    <div>
      <h2>{session.name}</h2>
      <p class="meta mono">{session.tool} · <span title={session.workdir}>{session.workdir}</span></p>
    </div>
    <StatusPill status={session.status} />
  </header>

  <div class="placeholder">
    <p><strong>Live terminal lands in phase 4.</strong></p>
    <p class="muted">
      This page will show the xterm.js stream of the tmux pane plus an input bar
      that calls <code>POST /api/sessions/{session.id}/send</code>.
    </p>
    <dl class="meta-list">
      <dt>tmux target</dt><dd class="mono">{session.tmux_target ?? '—'}</dd>
      <dt>created</dt><dd class="mono">{session.created_at}</dd>
      <dt>updated</dt><dd class="mono">{session.updated_at}</dd>
      <dt>flags</dt><dd class="mono">{session.flags.length ? session.flags.join(' ') : '—'}</dd>
    </dl>
  </div>
{/if}

<style>
  .back {
    display: inline-block;
    margin-bottom: 0.75rem;
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .back:hover { color: var(--text); }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 1.25rem;
  }
  h2 {
    font-family: var(--font-display);
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  .meta { margin: 0; color: var(--text-2); font-size: 0.85rem; }
  .mono { font-family: var(--font-mono); }
  .muted { color: var(--muted); }
  .placeholder {
    padding: 1.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .placeholder p { margin: 0 0 0.5rem; }
  .meta-list {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    margin: 1rem 0 0;
    font-size: 0.85rem;
  }
  dt { color: var(--muted); }
  dd { margin: 0; color: var(--text); word-break: break-all; }
  .error {
    padding: 0.8rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
  }
  code { font-family: var(--font-mono); }
</style>
