<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api, type Session } from '$lib/api';
  import { watchdog, loadWatchdog } from '$stores/watchdog';
  import Terminal from '$components/Terminal.svelte';
  import SessionRail from '$components/dashboard/SessionRail.svelte';
  import { ctxOf, fmtUptime } from '$lib/dashboard';

  /* -- session state ------------------------------------------------- */
  let session = $state<Session | null>(null);
  let error = $state<string | null>(null);
  let inputText = $state('');
  let sending = $state(false);
  let lifecycleBusy = $state<null | 'start' | 'stop' | 'kill' | 'delete' | 'yolo'>(null);
  let rawKeys = $state('');

  const YOLO_FLAG = '--dangerously-skip-permissions';
  const isYolo = $derived(session ? session.flags.includes(YOLO_FLAG) : false);
  const canToggleYolo = $derived(session ? (session.status === 'idle' || session.status === 'stopped') : false);
  const isRunning = $derived(session?.status === 'running');

  /** tmux key shorthands for the inline quick-keys row. */
  const QUICK_KEYS: Array<{ label: string; spec: string; title: string }> = [
    { label: '^C',   spec: 'C-c',    title: 'Ctrl-C (interrupt)' },
    { label: '^D',   spec: 'C-d',    title: 'Ctrl-D (eof)' },
    { label: '^L',   spec: 'C-l',    title: 'Ctrl-L (clear)' },
    { label: 'Esc',  spec: 'Escape', title: 'Escape' },
    { label: '⏎',    spec: 'Enter',  title: 'Enter (no text)' },
    { label: 'Tab',  spec: 'Tab',    title: 'Tab' },
    { label: '↑',    spec: 'Up',     title: 'Up' },
    { label: '↓',    spec: 'Down',   title: 'Down' }
  ];

  /* -- load + poll --------------------------------------------------- */
  let pollId: ReturnType<typeof setInterval> | null = null;

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
    loadWatchdog(50);
    pollId = setInterval(() => { reload(); loadWatchdog(50); }, 5000);
  });

  onDestroy(() => {
    if (pollId) clearInterval(pollId);
  });

  /* -- input + actions ---------------------------------------------- */
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

  function onInputKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendText();
    }
  }

  async function lifecycle(label: typeof lifecycleBusy, fn: () => Promise<unknown>) {
    if (!session) return;
    lifecycleBusy = label;
    error = null;
    try { await fn(); await reload(); }
    catch (e) { error = e instanceof Error ? e.message : String(e); }
    finally  { lifecycleBusy = null; }
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

  async function compactSession() {
    if (!session) return;
    try { await api.sendInput(session.id, { keys: '/compact', append_enter: true }); }
    catch (e) { error = e instanceof Error ? e.message : String(e); }
  }
</script>

<div class="page">
  <!-- Toolbar -->
  <div class="toolbar">
    <div class="tabs">
      <button type="button" class="tab active">Terminal</button>
      <button type="button" class="tab" disabled title="Coming soon">
        Diff <span class="badge">—</span>
      </button>
      <button type="button" class="tab" disabled title="Coming soon">Activity</button>
    </div>
    <span class="spacer"></span>
    {#if session?.tmux_target}
      <span class="pill"><span style="color: var(--fg-3);">pane:</span>&nbsp;{session.tmux_target}</span>
    {/if}
    {#if isYolo}
      <span class="pill" style="color: var(--amber); border-color: rgba(255,180,84,0.35);">⚡ yolo</span>
    {/if}
    <button type="button" class="tb-btn" onclick={() => window.open(`/sessions/${session?.id}`, '_blank')} disabled={!session}>
      Pop out
    </button>
    <button
      type="button"
      class="tb-btn primary"
      onclick={compactSession}
      disabled={!isRunning}
      title="Send /compact to reduce context"
    >
      Send /compact
    </button>
  </div>

  <!-- Terminal + rail -->
  <div class="row">
    <div class="term-host">
      {#if session?.tmux_target || session?.model || session?.workdir}
        <div class="term-bar">
          <span style="color: var(--fg-3);">panes:</span>
          <span class="pane active">{session?.tool ?? 'shell'}</span>
          <span class="spacer"></span>
          <span>{session?.workdir ?? ''}</span>
          {#if session?.model}
            <span style="color: var(--fg-3);">·</span>
            <span>{session.model}</span>
          {/if}
          {#if session?.tmux_target}
            <span style="color: var(--fg-3);">·</span>
            <span>{session.tmux_target}</span>
          {/if}
          {#if typeof session?.ctx === 'number'}
            <span style="color: var(--fg-3);">·</span>
            <span>ctx {ctxOf(session)}%</span>
          {/if}
        </div>
      {/if}

      <div class="term-shell">
        {#if session}
          <Terminal sessionId={session.id} />
        {:else if !error}
          <div class="muted">connecting…</div>
        {/if}
      </div>

      <!-- Live input row -->
      <form class="input" onsubmit={(e) => { e.preventDefault(); sendText(); }}>
        <span class="prompt">›</span>
        <input
          type="text"
          bind:value={inputText}
          placeholder={isRunning ? 'type a message and press Enter…' : 'session is not running'}
          disabled={!isRunning || sending}
          onkeydown={onInputKey}
          autocomplete="off"
          spellcheck="false"
        />
        <span class="right">
          {#each QUICK_KEYS as k (k.spec)}
            <button
              type="button"
              class="qk"
              onclick={() => sendKey(k.spec)}
              disabled={!isRunning}
              title={k.title}
            >{k.label}</button>
          {/each}
        </span>
      </form>
    </div>

    {#if session}
      <SessionRail s={session} feed={$watchdog.items} />
    {/if}
  </div>

  {#if error}
    <div class="error mono" role="alert">{error}</div>
  {/if}

  <!-- Lifecycle drawer (advanced controls — kept out of the way) -->
  {#if session}
    <details class="drawer">
      <summary class="micro">Lifecycle &amp; raw keys</summary>
      <div class="drawer-row">
        {#if isRunning}
          <button type="button" class="tb-btn" onclick={onStop}    disabled={lifecycleBusy !== null}>{lifecycleBusy === 'stop'  ? 'stopping…' : 'Stop'}</button>
          <button type="button" class="tb-btn" onclick={onKill}    disabled={lifecycleBusy !== null}>{lifecycleBusy === 'kill'  ? 'killing…'  : 'Kill'}</button>
        {:else}
          <button type="button" class="tb-btn primary" onclick={onStart} disabled={lifecycleBusy !== null}>{lifecycleBusy === 'start' ? 'starting…' : 'Start'}</button>
        {/if}
        {#if canToggleYolo}
          <button type="button" class="tb-btn" onclick={toggleYolo} disabled={lifecycleBusy !== null}>
            {lifecycleBusy === 'yolo' ? '…' : isYolo ? '⚡ yolo: on' : 'yolo: off'}
          </button>
        {/if}
        <button type="button" class="tb-btn" onclick={onDelete} disabled={lifecycleBusy !== null} style="color: var(--crash); border-color: rgba(221,0,0,0.35);">
          {lifecycleBusy === 'delete' ? 'removing…' : 'Remove'}
        </button>
        <span class="spacer"></span>
        <form class="raw" onsubmit={(e) => { e.preventDefault(); sendRaw(); }}>
          <input
            type="text"
            bind:value={rawKeys}
            placeholder="raw tmux key spec (M-x, S-Tab)…"
            disabled={!isRunning}
            autocomplete="off"
            spellcheck="false"
          />
          <button type="submit" class="tb-btn" disabled={!isRunning || !rawKeys.trim()}>Send keys</button>
        </form>
      </div>
      <dl class="meta-list mono">
        <dt>id</dt>      <dd>{session.id}</dd>
        <dt>created</dt> <dd>{session.created_at}</dd>
        <dt>uptime</dt>  <dd>{fmtUptime(session.uptime_seconds, session.created_at)}</dd>
        <dt>flags</dt>   <dd>{session.flags.length ? session.flags.join(' ') : '—'}</dd>
      </dl>
    </details>
  {/if}
</div>

<style>
  .page {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }
  .row {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  /* Wrap xterm so it fills the .term-host middle. xterm.js owns its
     own canvas; our padding mirrors the design's `.term { padding: 14px 18px 50px }`. */
  .term-shell {
    flex: 1;
    min-height: 0;
    padding: 8px 8px 50px;
    overflow: hidden;
    background: #050505;
    overscroll-behavior: contain;
  }
  .term-shell :global(.terminal),
  .term-shell :global(.xterm) {
    height: 100% !important;
  }
  .muted {
    padding: 14px 18px;
    font-family: var(--mono);
    color: var(--fg-3);
  }

  /* Live input row — .term-host .input is positioned absolutely-bottom
     in _design.css; this just styles the inputs and quick-key chips. */
  .input input {
    flex: 1;
    background: transparent;
    border: 0;
    outline: none;
    color: var(--fg);
    font: inherit;
    padding: 0;
  }
  .input input::placeholder { color: var(--fg-3); }
  .input .qk {
    padding: 1px 7px;
    border-radius: 3px;
    background: var(--bg);
    border: 1px solid var(--border-2);
    color: var(--fg-2);
    font-family: var(--mono);
    font-size: 10.5px;
    cursor: pointer;
  }
  .input .qk:hover:not(:disabled) { color: var(--fg); border-color: var(--fg-3); }
  .input .qk:disabled { opacity: 0.4; cursor: not-allowed; }

  .error {
    margin: 12px 16px;
    padding: 10px 14px;
    border: 1px solid rgba(255,85,85,0.45);
    background: rgba(255,85,85,0.07);
    color: var(--crash);
    border-radius: var(--radius);
    font-size: 12px;
  }

  .drawer {
    border-top: 1px solid var(--border);
    padding: 8px 16px;
    background: #0d0d0d;
  }
  .drawer summary {
    cursor: pointer;
    color: var(--fg-3);
    list-style: none;
  }
  .drawer summary:hover { color: var(--fg-2); }
  .drawer-row {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    flex-wrap: wrap;
    align-items: center;
  }
  .raw { display: flex; gap: 8px; }
  .raw input {
    height: 26px;
    padding: 0 10px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 11px;
    min-width: 240px;
  }
  .raw input:disabled { opacity: 0.45; cursor: not-allowed; }

  .meta-list {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 4px 12px;
    margin: 12px 0 4px;
    font-size: 11px;
  }
  .meta-list dt { color: var(--fg-3); text-transform: uppercase; letter-spacing: 0.04em; }
  .meta-list dd { margin: 0; color: var(--fg-2); word-break: break-all; }

  /* Stack the session rail beneath the terminal on tablets — keeps the
     plan/todos panel reachable without crushing the terminal width.
     SessionRail uses the global .rail class, so target it via :global. */
  @media (max-width: 1100px) {
    .row { flex-direction: column; }
    .row :global(.rail) {
      width: 100%;
      max-height: 320px;
      border-left: 0;
      border-top: 1px solid var(--border);
    }
  }

  @media (max-width: 720px) {
    /* Sticky route header so the Pop out / /compact actions stay
       reachable while the user scrolls the terminal. */
    :global(.toolbar) {
      position: sticky;
      top: 0;
      z-index: 5;
      background: color-mix(in srgb, var(--bg-chrome) 92%, transparent);
      backdrop-filter: blur(10px);
      -webkit-backdrop-filter: blur(10px);
    }
    /* Pop out only opens a new tab — useless on phones. */
    :global(.toolbar .tb-btn:not(.primary)) { display: none; }
    :global(.tabs .tab[disabled]) { display: none; }

    /* Terminal + rail vertical stack: rail is collapsible meta below
       the active terminal, never larger than 50dvh so it doesn't push
       the input row off-screen on a phone. */
    .row :global(.rail) {
      max-height: 50dvh;
    }

    /* Input row redesign: prompt + input own the full width on the
       primary line; quick-keys scroll horizontally as a chip rail
       below — works like an extension of the soft keyboard. */
    .input {
      flex-wrap: wrap;
      gap: 8px;
      padding: 8px 10px 10px;
      height: auto !important;
      min-height: 56px;
    }
    .input input {
      flex: 1 1 100%;
      order: 1;
      padding: 8px 10px;
      background: var(--bg-2);
      border: 1px solid var(--border-2);
      border-radius: 8px;
    }
    .input .prompt { display: none; }
    .input .right {
      flex: 1 1 100%;
      order: 2;
      display: flex;
      gap: 6px;
      flex-wrap: nowrap;
      overflow-x: auto;
      padding-bottom: 2px;
      -webkit-overflow-scrolling: touch;
      scrollbar-width: none;
    }
    .input .right::-webkit-scrollbar { display: none; }
    .input .qk {
      flex: 0 0 auto;
      padding: 8px 10px;
      font-size: 11px;
      min-height: 32px;
      min-width: 36px;
    }

    /* Term-shell: reduce desktop bottom padding (input row overlay area)
       on mobile since input is sticky below, not overlaid. */
    .term-shell {
      padding-bottom: 8px;
    }

    /* Input row: sticky above the MobileNav so it stays visible when
       the soft keyboard is open. */
    .input {
      position: sticky;
      bottom: 0;
      z-index: 4;
      background: var(--bg-chrome);
      border-top: 1px solid var(--border);
    }

    /* Quick-key chips: 44×44px minimum touch targets (WCAG). */
    .input .qk {
      min-height: 44px;
      min-width: 44px;
      padding: 8px 12px;
      font-size: 12px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
    }

    /* Term-bar context line scrolls horizontally — long workdir paths
       were getting clipped before. */
    :global(.term-host .term-bar) {
      overflow-x: auto;
      white-space: nowrap;
      scrollbar-width: none;
    }
    :global(.term-host .term-bar::-webkit-scrollbar) { display: none; }

    /* Lifecycle drawer becomes a clean stacked block on phone. */
    .drawer-row { gap: 6px; }
    .raw {
      flex: 1 1 100%;
      width: 100%;
    }
    .raw input { min-width: 0; flex: 1; }
    .meta-list { grid-template-columns: 80px 1fr; font-size: 11.5px; }
  }
</style>
