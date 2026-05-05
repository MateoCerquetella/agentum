<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import '@xterm/xterm/css/xterm.css';
  import { api } from '$lib/api';

  interface Props {
    sessionId: string;
    /** When true, keystrokes are NOT forwarded back to the pane. */
    readonly?: boolean;
    /** When true, paint a thinner border + smaller status pill — for canvas tiles. */
    compact?: boolean;
  }
  let { sessionId, readonly = false, compact = false }: Props = $props();

  let host: HTMLDivElement;
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let ws: WebSocket | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let connected = $state(false);
  let connectionMsg = $state<string | null>('connecting…');
  let focused = $state(false);
  const encoder = new TextEncoder();

  // Single canonical palette — see docs/DESIGN-SYSTEM.md.
  const TERM_THEME = {
    background: '#0b0b0b',
    foreground: '#b9b9b9',
    cursor: '#0052ef',
    cursorAccent: '#ffffff',
    selectionBackground: '#0052ef',
    selectionForeground: '#ffffff',
  };

  function sendBytes(data: ArrayBuffer | Uint8Array) {
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    ws.send(data);
  }

  onMount(() => {
    term = new Terminal({
      fontFamily:
        'JetBrains Mono, "SF Mono", Menlo, Consolas, monospace',
      fontSize: compact ? 12 : 13,
      lineHeight: 1.2,
      cursorBlink: true,
      scrollback: 5000,
      theme: TERM_THEME,
      allowProposedApi: false,
      convertEol: false,
      disableStdin: readonly
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    // Keystrokes from xterm → WS → tmux send-keys -H
    if (!readonly) {
      term.onData((data) => {
        sendBytes(encoder.encode(data));
      });
      // xterm emits binary for some sequences (e.g. mouse). Forward both.
      term.onBinary((data) => {
        const buf = new Uint8Array(data.length);
        for (let i = 0; i < data.length; i++) buf[i] = data.charCodeAt(i) & 0xff;
        sendBytes(buf);
      });
    }

    resizeObserver = new ResizeObserver(() => fit?.fit());
    resizeObserver.observe(host);

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
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    ws?.close();
    term?.dispose();
  });

  function focus() {
    term?.focus();
    focused = true;
  }
  function blur() { focused = false; }

  // Public method exposed via export (consumers can call .resize() after layout).
  export function refit() { fit?.fit(); }
  export function paste(text: string) {
    if (!ws || readonly) return;
    sendBytes(encoder.encode(text));
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="wrap"
  class:compact
  class:focused
  class:readonly
  onclick={focus}
>
  <div class="term" bind:this={host} onfocusout={blur}></div>
  {#if connectionMsg}
    <div class="status" class:err={!connected}>{connectionMsg}</div>
  {/if}
  {#if readonly}
    <div class="ro-badge" title="read-only — open the session to type">read-only</div>
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
    min-height: 0;
    display: flex;
    cursor: text;
    transition: border-color var(--transition, 150ms ease), box-shadow var(--transition, 150ms ease);
  }
  .wrap:not(.compact) { min-height: 360px; }
  .wrap.compact {
    padding: 0.35rem;
    border-radius: 8px;
  }
  .wrap.focused {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 25%, transparent);
  }
  .wrap.readonly { cursor: default; }
  .term {
    flex: 1;
    min-height: 0;
    width: 100%;
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
  .compact .status { top: 0.4rem; right: 0.5rem; font-size: 0.65rem; }
  .status.err { color: var(--danger); border-color: color-mix(in srgb, var(--danger) 35%, var(--border)); }

  .ro-badge {
    position: absolute;
    bottom: 0.5rem;
    right: 0.6rem;
    font-family: var(--font-mono);
    font-size: 0.65rem;
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    background: color-mix(in srgb, var(--warn, #c08400) 12%, var(--surface-2));
    color: var(--warn, #c08400);
    border: 1px solid color-mix(in srgb, var(--warn, #c08400) 35%, var(--border));
    pointer-events: none;
  }

  /* xterm.js needs the canvas to inherit theme bg via this hook */
  :global(.xterm-viewport) { background: transparent !important; }
</style>
