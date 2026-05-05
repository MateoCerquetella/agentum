<script lang="ts">
  import { theme, applyTheme, nextTheme, THEMES, type Theme } from '$stores/theme';

  const labels: Record<Theme, string> = {
    'terminal-dark': 'Terminal',
    paperlight: 'Paper',
    'obsidian-dark': 'Obsidian',
    system: 'Auto'
  };
</script>

<div class="theme-switcher" role="group" aria-label="Theme">
  <button
    class="cycle"
    title="Cycle theme"
    onclick={() => applyTheme(nextTheme($theme))}
  >
    {labels[$theme]}
  </button>

  <div class="dropdown">
    {#each THEMES as t}
      <button
        type="button"
        class:active={$theme === t}
        onclick={() => applyTheme(t)}
      >{labels[t]}</button>
    {/each}
  </div>
</div>

<style>
  .theme-switcher {
    position: relative;
  }
  .cycle {
    padding: 0.4rem 0.7rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    min-width: 4.5rem;
    text-align: center;
  }
  .cycle:hover {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .dropdown {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 14px rgba(0,0,0,0.18);
    padding: 0.25rem;
    display: none;
    flex-direction: column;
    min-width: 7rem;
    z-index: 20;
  }
  .theme-switcher:hover .dropdown,
  .theme-switcher:focus-within .dropdown {
    display: flex;
  }
  .dropdown button {
    text-align: left;
    padding: 0.4rem 0.6rem;
    border-radius: 4px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.8rem;
  }
  .dropdown button:hover { background: var(--surface-2); color: var(--text); }
  .dropdown button.active { color: var(--accent); }
</style>
