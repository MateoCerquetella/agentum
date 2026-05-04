<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import '@xterm/xterm/css/xterm.css';
  import { api } from '$lib/api';
  import { get } from 'svelte/store';
  import { theme as themeStore, type Theme } from '$stores/theme';

  interface Props {
    sessionId: string;
  }
  let { sessionId }: Props = $props();

  let host: HTMLDivElement;
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let ws: WebSocket | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let connected = $state(false);
  let connectionMsg = $state<string | null>('connecting…');

  function palette(t: Theme) {
    if (t === 'paperlight') {
      return { background: '#fdfaf3', foreground: '#1a1411', cursor: '#c2410c' };
    }
    return { background: '#0a0a0c', foreground: '#e8e8ec', cursor: '#ff8a4c' };
  }

  function applyPalette() {
    if (!term) return;
    let t: Theme = get(themeStore);
    if (t === 'system') {
      const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
      t = prefersDark ? 'terminal-dark' : 'paperlight';
    }
    term.options.theme = palette(t);
  }

  onMount(() => {
    term = new Terminal({
      fontFamily:
        'JetBrains Mono, "SF Mono", Menlo, Consolas, monospace',
      fontSize: 13,
      lineHeight: 1.2,
      cursorBlink: true,
      scrollback: 5000,
      theme: palette($themeStore),
      allowProposedApi: false,
      convertEol: false
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    resizeObserver = new ResizeObserver(() => fit?.fit());
    resizeObserver.observe(host);

    const themeUnsub = themeStore.subscribe(() => applyPalette());

    ws = new WebSocket(api.streamUrl(sessionId));
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      connected = true;
      connectionMsg = null;
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') {
        term?.write(`\x1b[33m${ev.data}\x1b[0m\r\n`);
        return;
      }
      if (ev.data instanceof ArrayBuffer) {
        term?.write(new Uint8Array(ev.data));
      }
    };
    ws.onclose = () => {
      connected = false;
      connectionMsg = 'stream closed';
      term?.write('\r\n\x1b[2m[stream closed]\x1b[0m\r\n');
    };
    ws.onerror = () => {
      connected = false;
      connectionMsg = 'stream error';
    };

    return () => themeUnsub();
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    ws?.close();
    term?.dispose();
  });
</script>

<div class="wrap">
  <div class="term" bind:this={host}></div>
  {#if connectionMsg}
    <div class="status" class:err={!connected}>{connectionMsg}</div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.5rem;
    flex: 1;
    min-height: 360px;
    display: flex;
  }
  .term {
    flex: 1;
    min-height: 0;
  }
  .status {
    position: absolute;
    top: 0.7rem;
    right: 0.9rem;
    font-family: var(--font-mono);
    font-size: 0.72rem;
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--muted);
    border: 1px solid var(--border);
  }
  .status.err { color: var(--danger); border-color: color-mix(in srgb, var(--danger) 35%, var(--border)); }

  /* xterm.js needs the canvas to inherit theme bg via this hook */
  :global(.xterm-viewport) { background: transparent !important; }
</style>
