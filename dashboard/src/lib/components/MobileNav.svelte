<script lang="ts">
  import { page } from '$app/state';
  import { openNewSession } from '$stores/newSession';

  /**
   * Bottom tab bar — only renders on phone-width viewports.
   * Mirrors a native iOS/Android nav so primary routes are reachable
   * without opening the side drawer. The center "+" launches
   * NewSessionDialog (the most common mobile action).
   */
  const path = $derived(page.url.pathname);
  function isActive(prefix: string): boolean {
    if (prefix === '/') return path === '/';
    return path.startsWith(prefix);
  }
</script>

<nav class="mobile-nav" aria-label="Primary">
  <a class="tab" class:on={isActive('/')} href="/" aria-label="Overview">
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
      <rect x="2" y="2" width="5" height="5" rx="1"/>
      <rect x="9" y="2" width="5" height="5" rx="1"/>
      <rect x="2" y="9" width="5" height="5" rx="1"/>
      <rect x="9" y="9" width="5" height="5" rx="1"/>
    </svg>
    <span>Home</span>
  </a>
  <a class="tab" class:on={isActive('/sessions')} href="/sessions" aria-label="Sessions">
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
      <rect x="2" y="3" width="12" height="10" rx="1.5"/>
      <path d="M2 6h12"/>
    </svg>
    <span>Sessions</span>
  </a>
  <button type="button" class="fab" onclick={openNewSession} aria-label="Spawn session">
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.9">
      <path d="M3 8h10M8 3v10" stroke-linecap="round"/>
    </svg>
  </button>
  <a class="tab" class:on={isActive('/terminals')} href="/terminals" aria-label="Terminals">
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
      <rect x="2" y="3" width="12" height="10" rx="1.5"/>
      <path d="M5 7l2 2-2 2" stroke-linecap="round" stroke-linejoin="round"/>
      <path d="M9 11h3" stroke-linecap="round"/>
    </svg>
    <span>Terms</span>
  </a>
  <a class="tab" class:on={isActive('/settings')} href="/settings" aria-label="Settings">
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
      <circle cx="8" cy="8" r="2.2"/>
      <path d="M8 1.5v2M8 12.5v2M14.5 8h-2M3.5 8h-2M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4M12.6 12.6l-1.4-1.4M4.8 4.8L3.4 3.4" stroke-linecap="round"/>
    </svg>
    <span>More</span>
  </a>
</nav>

<style>
  .mobile-nav {
    display: none;
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 45;
    height: calc(56px + env(safe-area-inset-bottom, 0px));
    padding-bottom: env(safe-area-inset-bottom, 0px);
    background: color-mix(in srgb, var(--bg-chrome) 92%, transparent);
    backdrop-filter: blur(14px) saturate(140%);
    -webkit-backdrop-filter: blur(14px) saturate(140%);
    border-top: 1px solid var(--border);
    align-items: center;
    justify-content: space-around;
    gap: 0;
  }
  .tab {
    flex: 1;
    height: 56px;
    display: inline-flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    color: var(--fg-3);
    text-decoration: none;
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    -webkit-tap-highlight-color: transparent;
    transition: color var(--t-hover);
  }
  .tab svg { width: 20px; height: 20px; }
  .tab span {
    font-size: 9.5px;
    line-height: 1;
  }
  .tab:hover { color: var(--fg-2); }
  .tab.on { color: var(--fg); }
  .tab.on svg { color: var(--cta); }

  /* Center FAB — primary action (spawn session). Sits above the bar so
     it reads as the dominant tap target on phones. */
  .fab {
    flex: 0 0 auto;
    width: 52px;
    height: 52px;
    margin: 0 6px;
    border-radius: 999px;
    background: var(--cta);
    color: #fff;
    border: 0;
    display: inline-grid;
    place-items: center;
    cursor: pointer;
    box-shadow:
      0 1px 0 rgba(255, 255, 255, 0.06) inset,
      0 8px 24px rgba(243, 100, 88, 0.35);
    transform: translateY(-10px);
    -webkit-tap-highlight-color: transparent;
    transition: transform var(--t-hover), filter var(--t-hover);
  }
  .fab svg { width: 22px; height: 22px; }
  .fab:active { transform: translateY(-10px) scale(0.96); }

  @media (max-width: 720px) {
    .mobile-nav { display: flex; }
  }
</style>
