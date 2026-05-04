<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api, type Session } from '$lib/api';
  import StatusPill from '$components/StatusPill.svelte';
  import Terminal from '$components/Terminal.svelte';

  let session = $state<Session | null>(null);
  let error = $state<string | null>(null);
  let inputText = $state('');
  let sending = $state(false);

  onMount(async () => {
    const id = page.params.id;
    if (!id) {
      error = 'missing session id';
      return;
    }
    try {
      session = await api.getSession(id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
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

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendText();
    }
  }
</script>

<a class="back" href="/">← all sessions</a>

{#if error}
  <div class="error">Failed to load: <code>{error}</code></div>
{:else if !session}
  <div class="muted">loading…</div>
{:else}
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
    <StatusPill status={session.status} />
  </header>

  {#if session.status !== 'running'}
    <div class="hint">
      Session is <strong>{session.status}</strong>. Start it from your terminal:
      <pre><code>agentum up {session.name}</code></pre>
    </div>
  {/if}

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
    <div class="actions">
      <button type="button" onclick={() => sendKey('C-c')} disabled={session.status !== 'running'} title="Send Ctrl-C">
        ^C
      </button>
      <button type="submit" class="primary" disabled={session.status !== 'running' || !inputText || sending}>
        Send
      </button>
    </div>
  </form>

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
    margin-bottom: 0.9rem;
  }
  h2 {
    font-family: var(--font-display);
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  .meta { margin: 0; color: var(--text-2); font-size: 0.85rem; }
  .mono { font-family: var(--font-mono); }
  .muted { color: var(--muted); }

  .hint {
    padding: 0.75rem 1rem;
    margin-bottom: 0.85rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-2);
    font-size: 0.85rem;
  }
  .hint pre {
    margin: 0.5rem 0 0;
    padding: 0.5rem 0.7rem;
    background: var(--surface-2);
    border-radius: 4px;
    font-family: var(--font-mono);
    color: var(--accent);
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

  .actions { display: flex; gap: 0.4rem; }
  .actions button {
    padding: 0.55rem 0.9rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .actions button:hover:not(:disabled) {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .actions button:disabled { opacity: 0.45; cursor: not-allowed; }
  .actions .primary {
    background: var(--accent);
    color: var(--bg);
    border-color: var(--accent);
  }

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
    padding: 0.8rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
  }
  code { font-family: var(--font-mono); }
</style>
