<script lang="ts">
  import { onMount } from 'svelte';
  import { authState, refreshAuth, setTokenAndRetry } from '$stores/auth';

  interface Props { children: import('svelte').Snippet }
  let { children }: Props = $props();

  let token = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);

  onMount(() => {
    refreshAuth();
  });

  async function submit(e: Event) {
    e.preventDefault();
    if (!token.trim()) return;
    submitting = true;
    error = null;
    try {
      await setTokenAndRetry(token);
      token = '';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      submitting = false;
    }
  }
</script>

{#if $authState === 'unknown'}
  <div class="full-screen muted">connecting…</div>
{:else if $authState === 'unreachable'}
  <div class="full-screen err">
    <div class="card">
      <h2>backend unreachable</h2>
      <p class="muted">Could not reach <code>/api/health</code> on this host.</p>
      <p class="muted">Make sure <code>agentum serve</code> is running, then refresh.</p>
    </div>
  </div>
{:else if $authState === 'needs-token'}
  <div class="full-screen">
    <form class="card" onsubmit={submit}>
      <h2>bearer token required</h2>
      <p class="muted">
        Find it on the host with <code>agentum auth show</code> (or rotate it
        with <code>agentum auth rotate</code>).
      </p>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="password"
        bind:value={token}
        placeholder="paste token…"
        autocomplete="off"
        spellcheck="false"
        autofocus
      />
      {#if error}<p class="err-msg">{error}</p>{/if}
      <button type="submit" class="primary" disabled={!token.trim() || submitting}>
        {submitting ? 'verifying…' : 'unlock'}
      </button>
    </form>
  </div>
{:else}
  {@render children()}
{/if}

<style>
  .full-screen {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    width: 100%;
    background: var(--bg);
    padding: 1rem;
  }
  .full-screen.err { color: var(--danger); }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1.5rem 1.7rem;
    max-width: 440px;
    width: 100%;
    box-shadow: 0 4px 22px rgba(0,0,0,0.18);
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1.15rem;
    color: var(--text);
  }
  .muted { color: var(--muted); margin: 0; font-size: 0.88rem; line-height: 1.45; }
  code { font-family: var(--font-mono); color: var(--accent); }
  input {
    padding: 0.6rem 0.8rem;
    background: var(--surface-2);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.9rem;
    margin-top: 0.4rem;
  }
  input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .primary {
    background: var(--accent);
    color: var(--bg);
    border: 0;
    border-radius: 6px;
    padding: 0.55rem 1rem;
    font-family: var(--font-mono);
    font-size: 0.9rem;
    align-self: flex-start;
    margin-top: 0.3rem;
    cursor: pointer;
  }
  .primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .err-msg {
    margin: 0;
    color: var(--danger);
    font-size: 0.85rem;
  }
</style>
