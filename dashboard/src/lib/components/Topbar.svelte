<script lang="ts">
  import { page } from '$app/state';
  import { openPalette } from '$stores/palette';
  import { toggleFullscreen } from '$stores/fullscreen';
  import { currentUser, logout } from '$stores/auth';
  import { sessions } from '$stores/sessions';
  import { get } from 'svelte/store';
  import EndpointSwitcher from './EndpointSwitcher.svelte';

  /** Optional override. When omitted, crumbs are derived from the route. */
  interface Props { crumbs?: string[]; }
  let { crumbs }: Props = $props();

  function defaultCrumbs(): string[] {
    const path = page.url.pathname;
    if (path === '/') return ['agentum', 'overview'];
    if (path.startsWith('/board')) return ['agentum', 'board'];
    if (path.startsWith('/sessions/')) {
      const id = path.split('/')[2];
      const s = get(sessions).items.find(x => x.id === id);
      if (s) return ['agentum', s.name, s.tool];
      return ['agentum', 'session'];
    }
    if (path.startsWith('/terminals')) return ['agentum', 'terminals'];
    if (path.startsWith('/settings')) return ['agentum', 'settings'];
    return ['agentum'];
  }

  const resolvedCrumbs = $derived(crumbs ?? defaultCrumbs());
  const initial = $derived(($currentUser?.[0] ?? 'A').toUpperCase());

  async function onLogout() {
    if (!confirm('Sign out of agentum?')) return;
    await logout();
  }
</script>

<header class="db-top">
  <a class="brand" href="/" aria-label="agentum home">
    <svg width="20" height="20" viewBox="0 0 22 22" fill="none">
      <rect x="1" y="1" width="20" height="20" rx="5" stroke="currentColor" stroke-width="1.4"/>
      <path d="M6.2 14.6L10.6 5.5L15 14.6" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>
      <path d="M7.7 11.6H13.5" stroke="var(--cta)" stroke-width="1.6" stroke-linecap="round"/>
    </svg>
    <span>agentum</span>
  </a>

  <nav class="crumbs" aria-label="Breadcrumb">
    {#each resolvedCrumbs as c, i (i)}
      <span class:leaf={i === resolvedCrumbs.length - 1}>{c}</span>
      {#if i < resolvedCrumbs.length - 1}<span class="sep">/</span>{/if}
    {/each}
  </nav>

  <span class="spacer"></span>

  <div class="right">
    <span class="desktop-only"><EndpointSwitcher /></span>
    <button
      type="button"
      class="iconbtn desktop-only"
      title="Command palette (⌘K)"
      aria-label="Open command palette"
      onclick={openPalette}
    >
      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
        <circle cx="7" cy="7" r="4.5"/>
        <path d="M10.5 10.5L13.5 13.5" stroke-linecap="round"/>
      </svg>
    </button>
    <button
      type="button"
      class="iconbtn desktop-only"
      title="Fullscreen (Shift+F)"
      aria-label="Toggle fullscreen"
      onclick={toggleFullscreen}
    >
      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 6V3h3M13 6V3h-3M3 10v3h3M13 10v3h-3"/>
      </svg>
    </button>
    {#if $currentUser}
      <button
        type="button"
        class="avatar"
        title={`Sign out (${$currentUser})`}
        aria-label={`Sign out ${$currentUser}`}
        onclick={onLogout}
      >
        {initial}
      </button>
    {/if}
  </div>
</header>

<style>
  /* On phone the brand text crowds the route, since the bottom bar
     already says where the user is. Keep just the logo glyph. */
  @media (max-width: 480px) {
    .brand :global(span) { display: none; }
  }
  /* Bigger tap target for the avatar on phone — matches Apple's 44pt
     guidance without ballooning the visual. */
  @media (max-width: 720px) {
    .right :global(.avatar) {
      width: 36px;
      height: 36px;
      font-size: 13px;
    }
  }
</style>
