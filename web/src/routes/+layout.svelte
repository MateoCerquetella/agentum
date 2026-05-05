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
    }
    if (e.key === '?' && !isTypingTarget(e.target)) {
      e.preventDefault();
      openShortcuts();
    }
  }

  onMount(() => {
    applyTheme(get(theme));
    window.addEventListener('keydown', onKey);
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
        <main>{@render children()}</main>
      </div>
      <ToastStack />
      <CommandPalette />
      <ShortcutSheet />
      <NewSessionDialog open={$newSessionOpen} onClose={closeNewSession} />
    </div>
  {/snippet}
</TokenGate>

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
