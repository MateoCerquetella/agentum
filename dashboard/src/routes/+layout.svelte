<script lang="ts">
  import { get } from 'svelte/store';
  import '../app.css';
  import Sidebar from '$components/Sidebar.svelte';
  import Topbar from '$components/Topbar.svelte';
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
    window.addEventListener('keydown', onKey);
    const unsub = authState.subscribe((s) => {
      if (s === 'ok') {
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
      window.removeEventListener('keydown', onKey);
      unsub();
    };
  });

  onDestroy(() => { stopEventBridge(); disconnectEvents(); });
</script>

<TokenGate>
  {#snippet children()}
    <div class="db shell" class:fullscreen={$fullscreen} class:wide={isWideRoute(page.url.pathname)}>
      {#if !$fullscreen}<Topbar />{/if}
      <div class="row">
        {#if !$fullscreen}<Sidebar />{/if}
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

  @media (max-width: 720px) {
    .row { flex-direction: column; }
    .row :global(.sb) { width: 100%; height: auto; border-right: 0; border-bottom: 1px solid var(--border); }
  }
</style>
