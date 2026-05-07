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

  // Tell the daemon (and through it tmux) about the current pane size so
  // the embedded TUI redraws into the right viewport. Without this tmux
  // clamps to its 80×24 default and characters overlap.
  //
  // Daemons before v0.6.7 don't advertise the `resize` capability and
  // forward unknown text frames to `tmux send-keys` — that's how a stale
  // server ends up typing `{"resize":…}` straight into the agent's
  // prompt. We probe `/api/health` once and silently downgrade if the
  // capability isn't there.
  let lastSentSize = { cols: 0, rows: 0 };
  let resizeSupported = false;
  function sendResize() {
    if (!resizeSupported) return;
    if (!ws || ws.readyState !== WebSocket.OPEN || !term) return;
    const cols = term.cols;
    const rows = term.rows;
    if (cols <= 0 || rows <= 0) return;
    if (cols === lastSentSize.cols && rows === lastSentSize.rows) return;
    lastSentSize = { cols, rows };
    ws.send(JSON.stringify({ resize: { cols, rows } }));
  }

  let destroyed = false;

  /** Bump font size on narrow viewports for thumb-readability. */
  const isMobile = typeof window !== 'undefined' && window.matchMedia('(max-width: 720px)').matches;

  onMount(async () => {
    term = new Terminal({
      fontFamily:
        'JetBrains Mono, "SF Mono", Menlo, Consolas, monospace',
      fontSize: compact ? 12 : isMobile ? 14 : 13,
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

    resizeObserver = new ResizeObserver(() => {
      fit?.fit();
      // fit triggers term.onResize, but emit explicitly too in case the
      // observer fires before the WS is open (we'll de-dupe by size).
      sendResize();
    });
    resizeObserver.observe(host);

    // xterm's own resize event fires on fit() — pipe it through so the
    // tmux pane mirrors whatever xterm just laid itself out as.
    term.onResize(() => sendResize());

    // Probe `resize` capability BEFORE opening the WS — not concurrently.
    // The server arms a 250 ms `INITIAL_RESIZE_WAIT` window from WS open
    // and `capture-pane`s tmux at whatever size it currently has if no
    // resize lands in time. Tmux is pre-sized to 132×40 (v0.6.18). If
    // the WS open beat the probe (the common case on localhost), the
    // first `sendResize()` in `ws.onopen` early-returned with
    // `resizeSupported = false`, the server timed out and snapped at
    // 132×40, xterm rendered those bytes at the host's width, and every
    // line wrapped wrong — Claude's sticky footer (`▶▶ bypass
    // permissions…`) reflowed into the middle of chat output, leading
    // characters got eaten at line edges. Serializing the probe here
    // costs one HTTP round-trip (single-digit ms on localhost) and
    // guarantees the very first thing the WS sees post-open is a
    // correctly-sized resize frame.
    try {
      const h = await api.health();
      resizeSupported = Array.isArray(h.capabilities) && h.capabilities.includes('resize');
    } catch { /* leave resizeSupported = false */ }
    if (destroyed) return;

    ws = new WebSocket(api.streamUrl(sessionId));
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      connected = true;
      connectionMsg = null;
      // Reset cache so the first send always fires once we know the size.
      lastSentSize = { cols: 0, rows: 0 };
      sendResize();
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
    destroyed = true;
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
  export function refit() {
    fit?.fit();
    sendResize();
  }
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

  @media (max-width: 880px) {
    .wrap:not(.compact) { min-height: 200px; min-height: 40dvh; }
  }
  @media (max-width: 720px) {
    .wrap:not(.compact) { min-height: 50dvh; }
  }
</style>
