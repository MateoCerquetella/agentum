<script lang="ts">
  import { toasts, dismissToast } from '$stores/events';
</script>

<div class="stack" role="region" aria-label="notifications">
  {#each $toasts as t (t.id)}
    <button
      type="button"
      class="toast"
      data-kind={t.kind}
      onclick={() => dismissToast(t.id)}
      title="dismiss"
    >
      <div class="title">{t.title}</div>
      {#if t.body}<div class="body">{t.body}</div>{/if}
    </button>
  {/each}
</div>

<style>
  .stack {
    position: fixed;
    top: 1rem;
    right: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    z-index: 50;
    max-width: min(360px, calc(100% - 2rem));
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    padding: 0.7rem 0.9rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 4px 18px rgba(0,0,0,0.18);
    color: var(--text);
    text-align: left;
    cursor: pointer;
    animation: slideIn 180ms ease-out;
  }
  .toast[data-kind="info"]  { border-left: 3px solid var(--accent); }
  .toast[data-kind="warn"]  { border-left: 3px solid var(--warn); }
  .toast[data-kind="error"] { border-left: 3px solid var(--danger); color: var(--danger); }
  .title {
    font-family: var(--font-mono);
    font-size: 0.85rem;
    font-weight: 600;
  }
  .body {
    font-family: var(--font-sans);
    font-size: 0.78rem;
    color: var(--text-2);
    line-height: 1.35;
  }
  @keyframes slideIn {
    from { opacity: 0; transform: translateX(8px); }
    to   { opacity: 1; transform: translateX(0);   }
  }
</style>
