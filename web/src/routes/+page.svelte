<script lang="ts">
  import { onMount } from 'svelte';
  import { sessions, loadSessions } from '$stores/sessions';
  import SessionCard from '$components/SessionCard.svelte';
  import EmptyState from '$components/EmptyState.svelte';
  import Skeleton from '$components/Skeleton.svelte';

  onMount(() => {
    loadSessions();
    const id = setInterval(loadSessions, 5000);
    return () => clearInterval(id);
  });
</script>

<section class="head">
  <div>
    <h2>Sessions</h2>
    <p class="muted">All registered AI agent sessions on this host.</p>
  </div>
  <button class="primary" disabled title="New-session dialog lands phase 4+">+ New</button>
</section>

{#if $sessions.error}
  <div class="error">Failed to load sessions: <code>{$sessions.error}</code></div>
{:else if $sessions.loading && $sessions.items.length === 0}
  <div class="grid"><Skeleton rows={6} height="5rem" /></div>
{:else if $sessions.items.length === 0}
  <EmptyState
    title="No sessions yet"
    body="Register one from your terminal:"
    cmd="agentum new alpha --tool claude --dir ~/projects/foo --up"
  />
{:else}
  <div class="grid">
    {#each $sessions.items as session (session.id)}
      <SessionCard {session} />
    {/each}
  </div>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    margin-bottom: 1.25rem;
    gap: 1rem;
    flex-wrap: wrap;
  }
  h2 {
    font-family: var(--font-display);
    font-weight: 600;
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  .muted { color: var(--muted); margin: 0; }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 0.9rem;
  }
  .primary {
    background: var(--accent);
    color: var(--bg);
    padding: 0.5rem 1rem;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
    opacity: 0.55;
    cursor: not-allowed;
  }
  .error {
    padding: 0.8rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    margin-bottom: 1rem;
  }
  .error code { font-family: var(--font-mono); color: var(--text); }
</style>
