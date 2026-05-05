<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api, type Session } from '$lib/api';
  import StatusPill from '$components/StatusPill.svelte';
  import Terminal from '$components/Terminal.svelte';

  let session = $state<Session | null>(null);
  let error = $state<string | null>(null);
  let inputText = $state('');
  let sending = $state(false);
  let lifecycleBusy = $state<null | 'start' | 'stop' | 'kill' | 'delete' | 'yolo'>(null);
  let rawKeys = $state('');

  const YOLO_FLAG = '--dangerously-skip-permissions';
  const isYolo = $derived(session ? session.flags.includes(YOLO_FLAG) : false);
  const canToggleYolo = $derived(session ? (session.status === 'idle' || session.status === 'stopped') : false);

  // Common tmux key specs surfaced as one-click buttons. The CLI exposes
  // `agentum keys <name> <spec>`; this is the same thing visually.
  const QUICK_KEYS: Array<{ label: string; spec: string; title: string }> = [
    { label: '^C',   spec: 'C-c',   title: 'Ctrl-C (interrupt)' },
    { label: '^D',   spec: 'C-d',   title: 'Ctrl-D (eof)' },
    { label: '^L',   spec: 'C-l',   title: 'Ctrl-L (clear)' },
    { label: 'Esc',  spec: 'Escape', title: 'Escape' },
    { label: '⏎',    spec: 'Enter', title: 'Enter (no text)' },
    { label: 'Tab',  spec: 'Tab',   title: 'Tab' },
    { label: '↑',    spec: 'Up',    title: 'Arrow Up' },
    { label: '↓',    spec: 'Down',  title: 'Arrow Down' }
  ];

  async function reload() {
    const id = page.params.id;
    if (!id) return;
    try {
      session = await api.getSession(id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    if (!page.params.id) {
      error = 'missing session id';
      return;
    }
    reload();
    const tick = setInterval(reload, 4000);
    return () => clearInterval(tick);
  });

  async function sendText() {
    if (!session || !inputText) return;
    const text = inputText;
    sending = true;
    try {
      await api.sendInput(session.id, { text, append_enter: true });
      inputText = '';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      sending = false;
    }
  }

  async function sendKey(spec: string) {
    if (!session) return;
    try {
      await api.sendInput(session.id, { keys: spec });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function sendRaw() {
    if (!session || !rawKeys.trim()) return;
    const spec = rawKeys.trim();
    try {
      await api.sendInput(session.id, { keys: spec });
      rawKeys = '';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendText();
    }
  }

  async function lifecycle(label: typeof lifecycleBusy, fn: () => Promise<unknown>) {
    if (!session) return;
    lifecycleBusy = label;
    error = null;
    try {
      await fn();
      await reload();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      lifecycleBusy = null;
    }
  }

  function onStart() { if (session) lifecycle('start', () => api.startSession(session!.id)); }
  function onStop()  { if (session) lifecycle('stop',  () => api.stopSession(session!.id)); }
  function onKill()  {
    if (!session) return;
    if (!confirm(`Force-kill "${session.name}"? Any in-flight work will be lost.`)) return;
    lifecycle('kill', () => api.killSession(session!.id));
  }
  async function toggleYolo() {
    if (!session || !canToggleYolo) return;
    lifecycleBusy = 'yolo';
    error = null;
    try {
      const newFlags = isYolo
        ? session.flags.filter(f => f !== YOLO_FLAG)
        : [...session.flags, YOLO_FLAG];
      await api.patchSession(session.id, { flags: newFlags });
      await reload();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      lifecycleBusy = null;
    }
  }
  async function onDelete() {
    if (!session) return;
    const force = session.status === 'running';
    const verb = force ? 'kill and remove' : 'remove';
    if (!confirm(`${verb} session "${session.name}"?`)) return;
    lifecycleBusy = 'delete';
    error = null;
    try {
      await api.deleteSession(session.id, force);
      goto('/');
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      lifecycleBusy = null;
    }
  }
</script>

<a class="back" href="/">← all sessions</a>

{#if error}
  <div class="error">{error}</div>
{/if}

{#if !session && !error}
  <div class="muted">loading…</div>
{:else if session}
  <header class="head">
    <div>
      <h2>{session.name}</h2>
      <p class="meta mono">
        {session.tool}
        ·
        <span title={session.workdir}>{session.workdir}</span>
        {#if session.tmux_target}
          ·
          <span class="muted" title="tmux target">{session.tmux_target}</span>
        {/if}
      </p>
    </div>
    <div class="head-badges">
      {#if isYolo}
        <span class="yolo-pill" title="YOLO mode — permissions auto-approved">⚡ YOLO</span>
      {/if}
      <StatusPill status={session.status} />
    </div>
  </header>

  <div class="lifecycle">
    {#if session.status === 'running'}
      <button onclick={onStop} disabled={lifecycleBusy !== null}>
        {lifecycleBusy === 'stop' ? 'stopping…' : 'stop'}
      </button>
      <button onclick={onKill} disabled={lifecycleBusy !== null}>
        {lifecycleBusy === 'kill' ? 'killing…' : 'kill'}
      </button>
    {:else}
      <button onclick={onStart} disabled={lifecycleBusy !== null}>
        {lifecycleBusy === 'start' ? 'starting…' : 'start'}
      </button>
    {/if}
    {#if canToggleYolo}
      <button
        class="yolo-toggle"
        class:active={isYolo}
        onclick={toggleYolo}
        disabled={lifecycleBusy !== null}
        title={isYolo ? 'Disable YOLO mode' : 'Enable YOLO mode'}
      >
        {lifecycleBusy === 'yolo' ? '…' : isYolo ? '⚡ yolo: on' : 'yolo: off'}
      </button>
    {/if}
    <button class="danger" onclick={onDelete} disabled={lifecycleBusy !== null}>
      {lifecycleBusy === 'delete' ? 'removing…' : 'remove'}
    </button>
  </div>

  <Terminal sessionId={session.id} />

  <form class="input-bar" onsubmit={(e) => { e.preventDefault(); sendText(); }}>
    <input
      type="text"
      bind:value={inputText}
      placeholder={session.status === 'running' ? 'Type a message and hit Enter…' : 'session is not running'}
      disabled={session.status !== 'running' || sending}
      onkeydown={onKey}
      autocomplete="off"
      spellcheck="false"
    />
    <button type="submit" class="primary" disabled={session.status !== 'running' || !inputText || sending}>
      Send
    </button>
  </form>

  <div class="keys">
    <div class="quick">
      {#each QUICK_KEYS as k (k.spec)}
        <button
          type="button"
          onclick={() => sendKey(k.spec)}
          disabled={session.status !== 'running'}
          title={k.title}
        >
          {k.label}
        </button>
      {/each}
    </div>
    <form class="raw" onsubmit={(e) => { e.preventDefault(); sendRaw(); }}>
      <input
        type="text"
        bind:value={rawKeys}
        placeholder="raw tmux key spec (e.g. M-x, S-Tab)…"
        disabled={session.status !== 'running'}
        autocomplete="off"
        spellcheck="false"
      />
      <button
        type="submit"
        disabled={session.status !== 'running' || !rawKeys.trim()}
        title="Send raw key sequence (CLI: agentum keys)"
      >
        send keys
      </button>
    </form>
  </div>

  <details class="diag">
    <summary class="muted">debug info</summary>
    <dl class="meta-list">
      <dt>id</dt><dd class="mono">{session.id}</dd>
      <dt>created</dt><dd class="mono">{session.created_at}</dd>
      <dt>updated</dt><dd class="mono">{session.updated_at}</dd>
      <dt>flags</dt><dd class="mono">{session.flags.length ? session.flags.join(' ') : '—'}</dd>
    </dl>
  </details>
{/if}

<style>
  .back {
    display: inline-block;
    margin-bottom: 0.75rem;
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .back:hover { color: var(--text); }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 0.6rem;
  }
  .head-badges {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-shrink: 0;
  }
  .yolo-pill {
    font-family: var(--font-mono);
    font-size: 0.7rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 2px 8px;
    border-radius: 5px;
    color: var(--bg);
    background: var(--warning, #e6a817);
  }
  h2 {
    font-family: var(--font-display);
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  .meta { margin: 0; color: var(--text-2); font-size: 0.85rem; }
  .mono { font-family: var(--font-mono); }
  .muted { color: var(--muted); }

  .lifecycle {
    display: flex;
    gap: 0.4rem;
    margin-bottom: 0.85rem;
  }
  .lifecycle button {
    padding: 0.4rem 0.85rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--surface);
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.8rem;
    cursor: pointer;
  }
  .lifecycle button:hover:not(:disabled) {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .lifecycle button:disabled { opacity: 0.5; cursor: not-allowed; }
  .lifecycle .danger:hover:not(:disabled) {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 40%, var(--border));
  }
  .lifecycle .yolo-toggle {
    padding: 0.4rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--surface);
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.75rem;
    cursor: pointer;
  }
  .lifecycle .yolo-toggle.active {
    color: var(--bg, #222);
    background: var(--warning, #e6a817);
    border-color: var(--warning, #e6a817);
  }
  .lifecycle .yolo-toggle:hover:not(:disabled) {
    border-color: color-mix(in srgb, var(--warning, #e6a817) 60%, var(--border));
  }

  .input-bar {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.7rem;
  }
  .input-bar input {
    flex: 1;
    padding: 0.55rem 0.8rem;
    font-family: var(--font-mono);
    font-size: 0.9rem;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .input-bar input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .input-bar input:disabled { opacity: 0.5; cursor: not-allowed; }
  .input-bar .primary {
    padding: 0.55rem 1.2rem;
    background: var(--accent);
    color: var(--bg);
    border: 1px solid var(--accent);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
    cursor: pointer;
  }
  .input-bar .primary:disabled { opacity: 0.45; cursor: not-allowed; }

  .keys {
    margin-top: 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .quick { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .quick button {
    padding: 0.3rem 0.6rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 5px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.78rem;
    cursor: pointer;
  }
  .quick button:hover:not(:disabled) {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .quick button:disabled { opacity: 0.45; cursor: not-allowed; }

  .raw { display: flex; gap: 0.4rem; }
  .raw input {
    flex: 1;
    padding: 0.4rem 0.7rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 5px;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.78rem;
  }
  .raw input:disabled { opacity: 0.45; cursor: not-allowed; }
  .raw button {
    padding: 0.4rem 0.85rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 5px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.78rem;
    cursor: pointer;
  }
  .raw button:hover:not(:disabled) {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .raw button:disabled { opacity: 0.45; cursor: not-allowed; }

  .diag {
    margin-top: 1.2rem;
    border-top: 1px solid var(--border);
    padding-top: 0.7rem;
  }
  .diag summary { cursor: pointer; font-family: var(--font-mono); font-size: 0.8rem; }
  .meta-list {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    margin: 0.7rem 0 0;
    font-size: 0.85rem;
  }
  dt { color: var(--muted); }
  dd { margin: 0; color: var(--text); word-break: break-all; }
  .error {
    padding: 0.6rem 0.85rem;
    margin-bottom: 0.8rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-family: var(--font-mono);
    font-size: 0.82rem;
    word-break: break-word;
  }
</style>
