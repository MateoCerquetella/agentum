<script lang="ts">
  import { get } from 'svelte/store';
  import '../app.css';
  import Sidebar from '$components/Sidebar.svelte';
  import Topbar from '$components/Topbar.svelte';
  import ConnectionBanner from '$components/ConnectionBanner.svelte';
  import MobileNav from '$components/MobileNav.svelte';
  import TokenGate from '$components/TokenGate.svelte';
  import ToastStack from '$components/ToastStack.svelte';
  import CommandPalette from '$components/CommandPalette.svelte';
  import ShortcutSheet from '$components/ShortcutSheet.svelte';
  import NewSessionDialog from '$components/NewSessionDialog.svelte';
  import { newSessionOpen, closeNewSession } from '$stores/newSession';
  import { authState } from '$stores/auth';
  import { connect as connectEvents, disconnect as disconnectEvents } from '$stores/events';
  import { startEventBridge, stopEventBridge } from '$stores/event-bridge';
  import { startHostMetrics } from '$stores/host';
  import { startAttentionBridge } from '$stores/attention';
  import { tweaks, applyTweaks } from '$stores/tweaks';
  import { startThemeBridge, pullPreferences } from '$stores/theme-bridge';
  import { get as getStore } from 'svelte/store';
  import {
    palette, togglePalette, closePalette,
    shortcuts, openShortcuts, closeShortcuts
  } from '$stores/palette';
  import { fullscreen, toggleFullscreen, exitFullscreen } from '$stores/fullscreen';
  import { page } from '$app/state';
  import { onMount, onDestroy } from 'svelte';

  // Routes that need the full viewport width (the canvas, etc.).
  function isWideRoute(path: string): boolean {
    return path.startsWith('/terminals');
  }

  interface Props { children: import('svelte').Snippet }
  let { children }: Props = $props();

  // Mobile drawer state — only matters at narrow viewports where the
  // sidebar is hidden by default. Desktop always renders the sidebar
  // inline so this state has no effect there.
  let drawerOpen = $state(false);
  function openDrawer()  { drawerOpen = true; }
  function closeDrawer() { drawerOpen = false; }
  // Close the drawer on route change so navigating from the menu
  // doesn't leave the overlay sitting on top of the destination page.
  $effect(() => {
    void page.url.pathname;
    drawerOpen = false;
  });

  function isTypingTarget(t: EventTarget | null): boolean {
    if (!(t instanceof Element)) return false;
    const tag = t.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
    if ((t as HTMLElement).isContentEditable) return true;
    if (t.closest('.cm-editor')) return true;
    return false;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'k' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      togglePalette();
      return;
    }
    if (e.key === 'Escape') {
      if (get(palette).open) { e.preventDefault(); closePalette(); return; }
      if (get(shortcuts).open) { e.preventDefault(); closeShortcuts(); return; }
      if (get(fullscreen)) { e.preventDefault(); exitFullscreen(); return; }
    }
    if (e.key === '?' && !isTypingTarget(e.target)) {
      e.preventDefault();
      openShortcuts();
    }
    // Shift+F (uppercase F) toggles fullscreen — keeps lowercase 'f' free
    // for typing in any focused terminal pane.
    if (e.key === 'F' && e.shiftKey && !isTypingTarget(e.target)) {
      e.preventDefault();
      toggleFullscreen();
    }
  }

  onMount(() => {
    // Push the persisted accent + density onto :root before first paint.
    applyTweaks(getStore(tweaks));
    // Capture-phase listener so Escape closes the palette/shortcuts even
    // when an embedded xterm.js pane has focus and would otherwise call
    // preventDefault on the keydown before it bubbles up.
    window.addEventListener('keydown', onKey, true);
    // Start the dashboard↔TUI theme bridge. `startThemeBridge` is
    // idempotent and safe pre-auth; `pullPreferences` no-ops when the
    // request fails (older daemon, unauthenticated, offline).
    startThemeBridge();
    const unsub = authState.subscribe((s) => {
      if (s === 'ok') {
        // Auth just landed — sync from the server so the active theme
        // matches whatever the TUI last persisted.
        void pullPreferences();
        connectEvents();
        startEventBridge();
        startHostMetrics();
        startAttentionBridge();
      } else {
        stopEventBridge();
        disconnectEvents();
      }
    });
    return () => {
      window.removeEventListener('keydown', onKey, true);
      unsub();
    };
  });

  onDestroy(() => { stopEventBridge(); disconnectEvents(); });
</script>

<TokenGate>
  {#snippet children()}
    <div class="db shell" class:fullscreen={$fullscreen} class:wide={isWideRoute(page.url.pathname)} class:drawer-open={drawerOpen}>
      {#if !$fullscreen}
        <button
          type="button"
          class="menu-btn"
          aria-label="Open menu"
          aria-expanded={drawerOpen}
          onclick={openDrawer}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round">
            <path d="M2.5 4h11M2.5 8h11M2.5 12h11"/>
          </svg>
        </button>
        <Topbar />
      {/if}
      <ConnectionBanner />
      <div class="row">
        {#if !$fullscreen}<Sidebar />{/if}
        {#if drawerOpen && !$fullscreen}
          <button
            type="button"
            class="drawer-scrim"
            aria-label="Close menu"
            onclick={closeDrawer}
          ></button>
        {/if}
        <div class="main">
          <main>{@render children()}</main>
        </div>
      </div>
      {#if $fullscreen}
        <button
          class="exit-fs"
          type="button"
          onclick={exitFullscreen}
          title="Exit fullscreen (Esc / Shift+F)"
        >
          ⤢ exit
        </button>
      {/if}
      {#if !$fullscreen}<MobileNav />{/if}
      <ToastStack />
      <CommandPalette />
      <ShortcutSheet />
      <NewSessionDialog open={$newSessionOpen} onClose={closeNewSession} />
    </div>
  {/snippet}
</TokenGate>

<style>
  /* Design layout: TopBar full-width on top, then flex row of
     [Sidebar | Main]. The .db utility class (from _design.css) handles
     the canvas chrome; locals tune the inner row + main. */
  .shell {
    height: 100vh;
    height: 100dvh;
    min-height: 100vh;
    /* Honor iOS notch / camera cutout on the top edge. The bottom edge
       is paid for by the MobileNav itself. */
    padding-top: env(safe-area-inset-top, 0px);
  }
  .row {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
  }

  /* Wide routes (canvas, etc.) — same as default now; kept for API. */
  .shell.wide main { padding: 0; }

  /* Fullscreen mode: zero chrome, page consumes the viewport. */
  .shell.fullscreen main {
    padding: 0;
    max-width: 100%;
  }
  .shell.fullscreen { padding-top: 0; }

  .exit-fs {
    position: fixed;
    top: 0.6rem;
    right: 0.7rem;
    z-index: 50;
    padding: 0.3rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: color-mix(in srgb, var(--surface) 80%, transparent);
    backdrop-filter: blur(6px);
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.72rem;
    cursor: pointer;
    opacity: 0.55;
    transition: opacity var(--transition, 150ms ease), color var(--transition, 150ms ease);
  }
  .exit-fs:hover { opacity: 1; color: var(--text); }

  /* Hamburger lives only on narrow viewports — desktop never renders
     it. Sits over the topbar's brand area at small sizes. */
  .menu-btn {
    display: none;
    position: fixed;
    top: calc(env(safe-area-inset-top, 0px) + 6px);
    left: 8px;
    z-index: 60;
    width: 44px;
    height: 44px;
    align-items: center;
    justify-content: center;
    border-radius: 8px;
    border: 1px solid var(--border-2);
    background: color-mix(in srgb, var(--surface) 85%, transparent);
    backdrop-filter: blur(6px);
    color: var(--fg-2);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
    transition: color var(--transition, 150ms ease), background var(--transition, 150ms ease);
  }
  .menu-btn:hover { color: var(--fg); background: var(--surface); }
  .menu-btn:active { transform: scale(0.96); }

  /* Backdrop behind the drawer — tap-anywhere-to-dismiss, full
     viewport, dim the underlying page. */
  .drawer-scrim {
    display: none;
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(3px);
    z-index: 40;
    border: 0;
    padding: 0;
    cursor: pointer;
    animation: scrim-in 180ms ease;
  }
  @keyframes scrim-in { from { opacity: 0; } to { opacity: 1; } }

  /* Mobile / tablet layout. Sidebar becomes a drawer that slides in
     from the left when the hamburger is tapped. The .row stops being
     a horizontal flexbox so .main takes the full width. */
  @media (max-width: 880px) {
    .menu-btn { display: inline-flex; }

    /* Push the topbar's first content over so the hamburger doesn't
       overlap the brand mark. _design.css owns .db-top so we override
       via :global to add left padding on small screens. */
    .shell :global(.db-top) {
      padding-left: 60px;
    }

    .row {
      flex-direction: row;
      position: relative;
    }
    /* Hide the sidebar inline; reveal it as a drawer when toggled. */
    .row :global(.sb) {
      position: fixed;
      top: 0;
      left: 0;
      bottom: 0;
      width: min(88vw, 340px);
      height: 100dvh;
      z-index: 50;
      border-right: 1px solid var(--border);
      transform: translateX(-100%);
      transition: transform 220ms cubic-bezier(0.2, 0.7, 0.2, 1);
      box-shadow: 0 0 32px rgba(0, 0, 0, 0.5);
      padding-top: env(safe-area-inset-top, 0px);
      padding-bottom: env(safe-area-inset-bottom, 0px);
    }
    .shell.drawer-open :global(.sb) { transform: translateX(0); }
    .shell.drawer-open .drawer-scrim { display: block; }
  }

  /* Phone: rework the chrome. Topbar turns into a slim, transparent
     route header; primary nav lives in the bottom MobileNav. */
  @media (max-width: 720px) {
    /* Reserve room above the bottom MobileNav so .main content never
       sits underneath. The nav is 56px + safe-area inset bottom. */
    .main {
      padding-bottom: calc(56px + env(safe-area-inset-bottom, 0px));
    }
    /* In fullscreen we don't show the bottom bar — drop the gutter. */
    .shell.fullscreen .main { padding-bottom: 0; }

    /* Slim topbar on phones; non-essential bits get hidden by Topbar. */
    .shell :global(.db-top) {
      height: 48px;
      padding-left: 60px;
      padding-right: 10px;
      gap: 8px;
    }
    /* Hide the breadcrumb trail on phones — the active nav item in
       the bottom MobileNav already says where the user is. */
    .shell :global(.db-top .crumbs) { display: none; }

    /* Toolbar (per-route action bar) tightens up. */
    .shell :global(.toolbar) {
      padding: 8px 12px;
      gap: 8px;
      flex-wrap: wrap;
    }
  }

  @media (max-width: 480px) {
    .shell :global(.db-top) {
      padding-left: 56px;
      padding-right: 6px;
    }
  }
</style>
