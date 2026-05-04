<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, type Channel, type Message, type Session } from '$lib/api';
  import { onEvent, type BusEvent } from '$stores/events';

  let channels = $state<Channel[]>([]);
  let sessions = $state<Session[]>([]);
  let active = $state<Channel | null>(null);
  let messages = $state<Message[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Composer
  let draft = $state('');
  let sender = $state<string>('');

  // Create-channel form
  let pickA = $state('');
  let pickB = $state('');
  let creating = $state(false);

  let unsub: (() => void) | null = null;
  let scrollEl = $state<HTMLDivElement | null>(null);

  onMount(async () => {
    await Promise.all([loadChannels(), loadSessions()]);
    unsub = onEvent(handleBusEvent);
  });
  onDestroy(() => unsub?.());

  async function loadChannels() {
    try {
      channels = await api.listChannels();
      if (!active && channels.length > 0) await selectChannel(channels[0]);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function loadSessions() {
    try {
      sessions = await api.listSessions();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function selectChannel(ch: Channel) {
    active = ch;
    messages = [];
    sender = ch.a_session;
    loading = true;
    try {
      messages = await api.listMessages(ch.id);
      scrollToBottom();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function handleBusEvent(ev: BusEvent) {
    if (ev.kind === 'message.posted' && active && ev.payload['channel_id'] === active.id) {
      const m: Message = {
        id: Number(ev.payload['message_id']),
        channel_id: Number(ev.payload['channel_id']),
        sender: String(ev.payload['sender']),
        body: String(ev.payload['body']),
        ts: String(ev.payload['ts'])
      };
      // De-dup if our own POST already pushed it.
      if (!messages.some((x) => x.id === m.id)) {
        messages = [...messages, m];
        scrollToBottom();
      }
    } else if (ev.kind === 'channel.created' || ev.kind === 'channel.deleted') {
      loadChannels();
    }
  }

  function scrollToBottom() {
    requestAnimationFrame(() => {
      if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;
    });
  }

  async function send(e: Event) {
    e.preventDefault();
    if (!active || !draft.trim() || !sender) return;
    const body = draft.trim();
    try {
      const m = await api.postMessage(active.id, { sender, body });
      // Optimistic local push (event will be deduped)
      if (!messages.some((x) => x.id === m.id)) {
        messages = [...messages, m];
        scrollToBottom();
      }
      draft = '';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function createChannel(e: Event) {
    e.preventDefault();
    if (!pickA || !pickB || pickA === pickB) {
      error = 'pick two different sessions';
      return;
    }
    creating = true;
    error = null;
    try {
      const ch = await api.createChannel({ a_session: pickA, b_session: pickB });
      pickA = '';
      pickB = '';
      await loadChannels();
      await selectChannel(ch);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      creating = false;
    }
  }

  async function removeChannel() {
    if (!active) return;
    if (!confirm('delete this channel?')) return;
    try {
      await api.deleteChannel(active.id);
      active = null;
      messages = [];
      await loadChannels();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function sessionLabel(id: string): string {
    const s = sessions.find((x) => x.id === id);
    return s ? s.name : id.slice(0, 8) + '…';
  }

  function fmtTime(ts: string): string {
    const d = new Date(ts);
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }
</script>

<section class="head">
  <div>
    <h2>Channels</h2>
    <p class="muted">1:1 inter-session messaging. Live updates over /api/events.</p>
  </div>
  <form class="create" onsubmit={createChannel}>
    <select bind:value={pickA} disabled={sessions.length < 2}>
      <option value="">— session A —</option>
      {#each sessions as s}<option value={s.id}>{s.name}</option>{/each}
    </select>
    <select bind:value={pickB} disabled={sessions.length < 2}>
      <option value="">— session B —</option>
      {#each sessions as s}<option value={s.id}>{s.name}</option>{/each}
    </select>
    <button class="primary" type="submit" disabled={!pickA || !pickB || pickA === pickB || creating}>
      + new
    </button>
  </form>
</section>

{#if error}
  <div class="error">{error}</div>
{/if}

<div class="layout">
  <aside class="list">
    {#if channels.length === 0}
      <p class="muted">
        No channels yet. Need at least 2 sessions registered, then create one.
      </p>
    {:else}
      {#each channels as ch (ch.id)}
        <button
          type="button"
          class="row"
          class:active={active?.id === ch.id}
          onclick={() => selectChannel(ch)}
        >
          <div class="row-title">
            {sessionLabel(ch.a_session)} ⇄ {sessionLabel(ch.b_session)}
          </div>
          <div class="row-meta mono">id={ch.id}</div>
        </button>
      {/each}
    {/if}
  </aside>

  <section class="thread">
    {#if !active}
      <div class="empty muted">Select a channel.</div>
    {:else}
      <header class="thread-head">
        <div>
          <div class="title">
            {sessionLabel(active.a_session)} ⇄ {sessionLabel(active.b_session)}
          </div>
          <div class="muted mono">channel #{active.id}</div>
        </div>
        <button type="button" class="ghost danger" onclick={removeChannel}>delete</button>
      </header>

      <div class="messages" bind:this={scrollEl}>
        {#if loading}<div class="muted">loading…</div>
        {:else if messages.length === 0}<div class="muted">no messages yet</div>
        {/if}
        {#each messages as m (m.id)}
          <div class="msg" class:mine={m.sender === sender}>
            <div class="meta mono">
              <span>{sessionLabel(m.sender)}</span>
              <span class="time">{fmtTime(m.ts)}</span>
            </div>
            <div class="body">{m.body}</div>
          </div>
        {/each}
      </div>

      <form class="composer" onsubmit={send}>
        <select bind:value={sender} title="send as">
          <option value={active.a_session}>as {sessionLabel(active.a_session)}</option>
          <option value={active.b_session}>as {sessionLabel(active.b_session)}</option>
        </select>
        <input
          type="text"
          bind:value={draft}
          placeholder="message…"
          autocomplete="off"
        />
        <button type="submit" class="primary" disabled={!draft.trim()}>send</button>
      </form>
    {/if}
  </section>
</div>

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 1rem;
    margin-bottom: 1rem;
    flex-wrap: wrap;
  }
  h2 {
    margin: 0 0 0.25rem;
    font-family: var(--font-display);
    font-size: 1.4rem;
    font-weight: 600;
  }
  .muted { color: var(--muted); margin: 0; }

  .create {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .create select, .composer select {
    padding: 0.45rem 0.7rem;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
    min-width: 7rem;
  }
  .primary {
    background: var(--accent);
    color: var(--bg);
    padding: 0.45rem 0.9rem;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
    border: 0;
    cursor: pointer;
  }
  .primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .ghost {
    padding: 0.4rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.8rem;
  }
  .ghost.danger:hover { color: var(--danger); border-color: var(--danger); }

  .error {
    padding: 0.7rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-family: var(--font-mono);
    font-size: 0.85rem;
    margin-bottom: 0.6rem;
  }

  .layout {
    display: grid;
    grid-template-columns: 250px 1fr;
    gap: 0.85rem;
    min-height: 0;
    flex: 1;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    padding: 0.4rem;
    overflow: auto;
  }
  .row {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    text-align: left;
    padding: 0.5rem 0.6rem;
    border-radius: 6px;
    color: var(--text);
    cursor: pointer;
  }
  .row:hover { background: var(--surface-2); }
  .row.active {
    background: color-mix(in srgb, var(--accent) 14%, var(--surface-2));
  }
  .row-title { font-weight: 600; font-size: 0.9rem; }
  .row-meta { color: var(--muted); font-size: 0.72rem; }
  .mono { font-family: var(--font-mono); }

  .thread {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
    min-height: 0;
  }
  .thread-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.6rem 0.9rem;
    border-bottom: 1px solid var(--border);
  }
  .thread-head .title {
    font-family: var(--font-display);
    font-weight: 600;
    font-size: 1rem;
  }

  .messages {
    flex: 1;
    overflow: auto;
    padding: 0.7rem 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-height: 200px;
  }
  .msg {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    max-width: 80%;
    padding: 0.55rem 0.75rem;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    align-self: flex-start;
  }
  .msg.mine {
    align-self: flex-end;
    background: color-mix(in srgb, var(--accent) 12%, var(--surface-2));
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .meta {
    display: flex;
    justify-content: space-between;
    gap: 0.6rem;
    font-size: 0.72rem;
    color: var(--muted);
  }
  .time { color: var(--muted); }
  .body {
    font-size: 0.92rem;
    color: var(--text);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .composer {
    display: flex;
    gap: 0.5rem;
    padding: 0.6rem 0.9rem;
    border-top: 1px solid var(--border);
    background: var(--surface-2);
  }
  .composer input {
    flex: 1;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface);
    color: var(--text);
    font-family: var(--font-sans);
  }
  .composer input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }

  .empty {
    text-align: center;
    padding: 4rem 1rem;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
</style>
