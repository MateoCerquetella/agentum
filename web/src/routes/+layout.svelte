<script lang="ts">
  import { get } from 'svelte/store';
  import '../app.css';
  import Sidebar from '$components/Sidebar.svelte';
  import Topbar from '$components/Topbar.svelte';
  import TokenGate from '$components/TokenGate.svelte';
  import ToastStack from '$components/ToastStack.svelte';
  import CommandPalette from '$components/CommandPalette.svelte';
  import ShortcutSheet from '$components/ShortcutSheet.svelte';
  import { theme, applyTheme } from '$stores/theme';
  import { authState } from '$stores/auth';
  import { connect as connectEvents, disconnect as disconnectEvents } from '$stores/events';
  import {
    palette, togglePalette, closePalette,
    shortcuts, openShortcuts, closeShortcuts
  } from '$stores/palette';
  import { onMount, onDestroy } from 'svelte';

  interface Props { children: import('svelte').Snippet }
  let { children }: Props = $props();

  function isTypingTarget(t: EventTarget | null): boolean {
    if (!(t instanceof Element)) return false;
    const tag = t.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
    if ((t as HTMLElement).isContentEditable) return true;
    if (t.closest('.cm-editor')) return true; // CodeMirror
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
    }
    if (e.key === '?' && !isTypingTarget(e.target)) {
      e.preventDefault();
      openShortcuts();
    }
  }

  function registerServiceWorker() {
    if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return;
    if (location.hostname === 'localhost' && import.meta.env?.DEV) return;
    navigator.serviceWorker
      .register('/service-worker.js', { scope: '/' })
      .catch((e) => console.warn('service worker register failed:', e));
  }

  onMount(() => {
    // Re-apply on mount so the persisted theme wins over the hard-coded
    // app.html attribute.
    applyTheme(get(theme));
    registerServiceWorker();
    window.addEventListener('keydown', onKey);
    // Open the events bus once auth is OK; reconnect if state cycles back.
    const unsub = authState.subscribe((s) => {
      if (s === 'ok') connectEvents();
      else disconnectEvents();
    });
    return () => {
      window.removeEventListener('keydown', onKey);
      unsub();
    };
  });

  onDestroy(() => disconnectEvents());
</script>

<TokenGate>
  {#snippet children()}
    <div class="shell">
      <Sidebar />
      <div class="main">
        <Topbar />
        <main>{@render originalChildren()}</main>
      </div>
      <ToastStack />
      <CommandPalette />
      <ShortcutSheet />
    </div>
  {/snippet}
</TokenGate>

{#snippet originalChildren()}
  {@render children()}
{/snippet}

<style>
  .shell {
    display: flex;
    min-height: 100vh;
  }
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  main {
    flex: 1;
    padding: 1.5rem 1.75rem;
    max-width: 1100px;
    width: 100%;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  @media (max-width: 720px) {
    .shell { flex-direction: column; }
    main { padding: 1rem; }
  }
</style>
