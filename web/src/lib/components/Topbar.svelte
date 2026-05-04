<script lang="ts">
  import ThemeSwitcher from './ThemeSwitcher.svelte';

  interface Props { title?: string }
  let { title = 'agentum' }: Props = $props();
</script>

<header class="topbar">
  <h1>{title}</h1>
  <div class="actions">
    <button class="ghost" disabled title="Command palette (phase 9)">⌘K</button>
    <ThemeSwitcher />
  </div>
</header>

<style>
  .topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.85rem 1.25rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
    position: sticky;
    top: 0;
    z-index: 5;
  }
  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1.05rem;
    font-weight: 600;
    color: var(--text);
    letter-spacing: -0.005em;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  .ghost {
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    cursor: not-allowed;
  }
</style>
