<script lang="ts">
  import Icon from './Icon.svelte';
  import { openPalette } from '$stores/palette';
  import { toggleFullscreen } from '$stores/fullscreen';
  import { currentUser, logout } from '$stores/auth';
  import { page } from '$app/state';

  async function onLogout() {
    if (!confirm('Sign out of agentum?')) return;
    await logout();
  }

  interface Props { title?: string }
  let { title = 'agentum' }: Props = $props();

  function pageLabel(): string {
    const p = page.url.pathname;
    if (p === '/' || p.startsWith('/sessions')) return 'Agents';
    if (p.startsWith('/terminals')) return 'Terminals';
    if (p.startsWith('/settings')) return 'Settings';
    return 'agentum';
  }
</script>

<header class="topbar">
  <div class="left">
    <span class="breadcrumb">
      <span class="bc-root">agentum</span>
      <span class="bc-sep">/</span>
      <span class="bc-page">{pageLabel()}</span>
    </span>
  </div>
  <div class="actions">
    <button class="ghost" type="button" onclick={openPalette} title="Command palette (⌘K)">
      <Icon name="search" size={14} />
      <span class="kb-hint">⌘K</span>
    </button>
    <button
      class="ghost"
      type="button"
      onclick={toggleFullscreen}
      title="Fullscreen (Shift+F)"
    >
      ⤢
      <span class="kb-hint">⇧F</span>
    </button>
    {#if $currentUser}
      <button class="user" type="button" onclick={onLogout} title="Sign out">
        <Icon name="user" size={14} />
        <span class="uname">{$currentUser}</span>
      </button>
    {/if}
  </div>
</header>

<style>
  .topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 1.25rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
    position: sticky;
    top: 0;
    z-index: 5;
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
  }
  .left { display: flex; align-items: center; gap: 0.6rem; }
  .breadcrumb {
    font-size: 0.82rem;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .bc-root {
    font-family: var(--font-display);
    font-weight: 600;
    color: var(--text);
  }
  .bc-sep { color: var(--muted); font-family: var(--font-mono); }
  .bc-page {
    font-family: var(--font-mono);
    color: var(--accent);
    font-weight: 500;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .ghost {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.35rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    cursor: pointer;
    background: var(--surface-2);
    transition: border-color var(--transition, 150ms ease), color var(--transition, 150ms ease);
  }
  .ghost:hover {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .kb-hint {
    font-size: 0.72rem;
    color: var(--muted);
    background: var(--surface);
    padding: 0.05em 0.35em;
    border-radius: 3px;
    border: 1px solid var(--border);
  }
  .user {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.35rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.78rem;
    cursor: pointer;
    transition: border-color var(--transition, 150ms ease), color var(--transition, 150ms ease);
  }
  .user:hover {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .uname { letter-spacing: 0.02em; }
</style>
