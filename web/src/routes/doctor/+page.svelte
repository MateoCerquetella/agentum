<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type DoctorReport } from '$lib/api';

  let report = $state<DoctorReport | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);

  async function refresh() {
    loading = true;
    error = null;
    try {
      report = await api.doctor();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(refresh);
</script>

<section class="head">
  <div>
    <h2>Doctor</h2>
    <p class="muted">Same checks as <code>agentum doctor</code> on the host running the server.</p>
  </div>
  <button class="ghost" onclick={refresh} disabled={loading}>
    {loading ? 'checking…' : 'recheck'}
  </button>
</section>

{#if error}
  <div class="error">{error}</div>
{:else if !report && loading}
  <p class="muted">running checks…</p>
{:else if report}
  <div class="summary" class:ok={report.ok} class:bad={!report.ok}>
    {#if report.ok}
      <strong>all checks passed</strong>
    {:else}
      <strong>{report.failures} problem{report.failures === 1 ? '' : 's'} found</strong>
    {/if}
  </div>

  <ul class="checks">
    {#each report.checks as c (c.label)}
      <li class:ok={c.passed} class:bad={!c.passed}>
        <span class="icon" aria-hidden="true">{c.passed ? '✓' : '✗'}</span>
        <span class="label">{c.label}</span>
        <span class="detail mono">{c.detail}</span>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    margin-bottom: 1rem;
    gap: 1rem;
    flex-wrap: wrap;
  }
  h2 {
    font-family: var(--font-display);
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  .muted { color: var(--muted); margin: 0; }
  .muted code { font-family: var(--font-mono); color: var(--accent); }

  .ghost {
    padding: 0.45rem 0.85rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    cursor: pointer;
  }
  .ghost:hover:not(:disabled) {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .ghost:disabled { opacity: 0.55; cursor: not-allowed; }

  .summary {
    padding: 0.7rem 1rem;
    border-radius: var(--radius);
    margin-bottom: 0.9rem;
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .summary.ok {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    color: var(--accent);
    border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .summary.bad {
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    border: 1px solid color-mix(in srgb, var(--danger) 40%, var(--border));
  }

  .checks {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .checks li {
    display: grid;
    grid-template-columns: 1.5rem 8rem 1fr;
    gap: 0.6rem;
    align-items: center;
    padding: 0.6rem 0.85rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .checks li.ok .icon { color: var(--accent); }
  .checks li.bad .icon { color: var(--danger); }
  .icon { font-family: var(--font-mono); font-size: 1rem; text-align: center; }
  .label { color: var(--text); font-size: 0.85rem; }
  .detail { color: var(--text-2); font-size: 0.78rem; word-break: break-all; }
  .mono { font-family: var(--font-mono); }

  .error {
    padding: 0.8rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
</style>
