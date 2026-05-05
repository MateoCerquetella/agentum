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
    <nav class="breadcrumb mono" aria-label="Breadcrumb">
      <span class="crumb root">agentum</span>
      <span class="sep">/</span>
      <span class="crumb leaf">{pageLabel()}</span>
    </nav>
  </div>

  <div class="actions">
    <button class="action" type="button" onclick={openPalette} title="Command palette (⌘K)">
      <Icon name="search" size={14} />
      <kbd class="mono">⌘K</kbd>
    </button>
    <button
      class="action icon-only"
      type="button"
      onclick={toggleFullscreen}
      title="Fullscreen (Shift+F)"
      aria-label="Toggle fullscreen"
    >
      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 6V3h3M13 6V3h-3M3 10v3h3M13 10v3h-3" />
      </svg>
    </button>
    {#if $currentUser}
      <button class="user mono" type="button" onclick={onLogout} title="Sign out">
        <span class="user-dot" aria-hidden="true"></span>
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
    padding: 14px 24px;
    border-bottom: 1px solid var(--border-2);
    background: rgba(11, 11, 11, 0.78);
    backdrop-filter: saturate(180%) blur(18px);
    -webkit-backdrop-filter: saturate(180%) blur(18px);
    position: sticky;
    top: 0;
    z-index: 5;
  }

  .breadcrumb {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    font-size: 12.5px;
    letter-spacing: 0.02em;
  }
  .crumb { display: inline-flex; align-items: center; }
  .crumb.root { color: var(--text-2); }
  .crumb.leaf { color: var(--accent); }
  .sep { color: var(--muted); }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .action {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 10px;
    border-radius: var(--radius-sm);
    color: var(--text-2);
    background: var(--surface);
    border: 1px solid var(--border-2);
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
  }
  .action:hover {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }
  .action.icon-only { padding: 0 9px; }
  kbd {
    font-size: 10.5px;
    line-height: 1;
    padding: 3px 5px;
    border-radius: 4px;
    background: var(--bg);
    border: 1px solid var(--border-2);
    color: var(--muted);
    transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
  }
  .action:hover kbd {
    background: rgba(255, 255, 255, 0.12);
    color: #fff;
    border-color: rgba(255, 255, 255, 0.2);
  }

  .user {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 12px;
    border-radius: 99999px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    color: var(--text-2);
    font-size: 12px;
    cursor: pointer;
    letter-spacing: 0.02em;
    transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
  }
  .user:hover {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .user-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--success);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--success) 25%, transparent);
  }
  .uname { letter-spacing: 0.04em; }

  @media (max-width: 720px) {
    .topbar { padding: 10px 14px; }
    .breadcrumb { font-size: 11.5px; }
    .action { padding: 0 8px; }
  }
</style>
