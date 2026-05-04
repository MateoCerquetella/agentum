<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { api, type Note } from '$lib/api';
  import { notes, loadNotes } from '$stores/notes';
  import { theme as themeStore } from '$stores/theme';
  import NoteEditor from '$components/NoteEditor.svelte';

  let active = $state<Note | null>(null);
  let titleDraft = $state('');
  let bodyDraft = $state('');
  let saveStatus = $state<'idle' | 'pending' | 'saving' | 'saved' | 'error'>('idle');
  let saveError = $state<string | null>(null);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  let dark = $derived.by(() => {
    const t = $themeStore;
    if (t === 'paperlight') return false;
    if (t === 'terminal-dark') return true;
    if (typeof window !== 'undefined') {
      return window.matchMedia('(prefers-color-scheme: dark)').matches;
    }
    return true;
  });

  onMount(loadNotes);

  function pickFirstIfNone() {
    if (!active) {
      const v = get(notes);
      if (v.items.length > 0) selectNote(v.items[0]);
    }
  }
  $effect(() => {
    pickFirstIfNone();
  });

  function selectNote(n: Note) {
    active = n;
    titleDraft = n.title;
    bodyDraft = n.body;
    saveStatus = 'idle';
    saveError = null;
  }

  async function createBlank() {
    try {
      const n = await api.createNote({ title: 'Untitled', body: '' });
      await loadNotes();
      selectNote(n);
    } catch (e) {
      saveError = e instanceof Error ? e.message : String(e);
    }
  }

  async function deleteActive() {
    if (!active) return;
    if (!confirm(`Delete "${active.title}"?`)) return;
    try {
      await api.deleteNote(active.id);
      active = null;
      await loadNotes();
    } catch (e) {
      saveError = e instanceof Error ? e.message : String(e);
    }
  }

  function scheduleSave() {
    if (!active) return;
    saveStatus = 'pending';
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(flushSave, 800);
  }

  async function flushSave() {
    if (!active) return;
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    if (titleDraft === active.title && bodyDraft === active.body) {
      saveStatus = 'saved';
      return;
    }
    saveStatus = 'saving';
    saveError = null;
    try {
      const updated = await api.patchNote(active.id, {
        title: titleDraft,
        body: bodyDraft
      });
      active = updated;
      saveStatus = 'saved';
      // Refresh the sidebar list so updated_at re-sorts.
      loadNotes();
    } catch (e) {
      saveStatus = 'error';
      saveError = e instanceof Error ? e.message : String(e);
    }
  }

  function onTitleChange() {
    if (!active) return;
    scheduleSave();
  }

  function onBodyChange(next: string) {
    if (!active) return;
    bodyDraft = next;
    scheduleSave();
  }
</script>

<section class="head">
  <div>
    <h2>Notes</h2>
    <p class="muted">Markdown notebook. Auto-saves on blur or after a brief pause.</p>
  </div>
  <button type="button" class="primary" onclick={createBlank}>+ new note</button>
</section>

{#if $notes.error}
  <div class="error">Failed to load: <code>{$notes.error}</code></div>
{/if}

<div class="layout">
  <aside class="list">
    {#if $notes.loading && $notes.items.length === 0}
      <div class="muted">loading…</div>
    {:else if $notes.items.length === 0}
      <p class="muted">No notes yet — capture your first thought.</p>
    {:else}
      {#each $notes.items as n (n.id)}
        <button
          type="button"
          class="row"
          class:active={active?.id === n.id}
          onclick={() => selectNote(n)}
        >
          <div class="row-title">{n.title || 'untitled'}</div>
          <div class="row-snippet mono">
            {n.body.slice(0, 80) || '—'}
          </div>
        </button>
      {/each}
    {/if}
  </aside>

  <section class="editor-pane">
    {#if !active}
      <div class="empty muted">
        Select a note from the left, or click <strong>+ new note</strong>.
      </div>
    {:else}
      <header class="editor-head">
        <input
          type="text"
          bind:value={titleDraft}
          oninput={onTitleChange}
          onblur={flushSave}
          placeholder="title"
          maxlength="200"
        />
        <div class="status mono" data-state={saveStatus}>
          {#if saveStatus === 'pending'}● editing{:else if saveStatus === 'saving'}saving…{:else if saveStatus === 'saved'}saved{:else if saveStatus === 'error'}error{:else}—{/if}
        </div>
        <button type="button" class="ghost danger" onclick={deleteActive}>delete</button>
      </header>
      {#if saveError}<div class="error inline">{saveError}</div>{/if}
      <NoteEditor value={bodyDraft} onchange={onBodyChange} onflush={flushSave} {dark} />
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
  .primary {
    background: var(--accent);
    color: var(--bg);
    padding: 0.45rem 0.9rem;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
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
  .error.inline { margin-bottom: 0.4rem; }
  code { font-family: var(--font-mono); }

  .layout {
    display: grid;
    grid-template-columns: 250px 1fr;
    gap: 0.85rem;
    flex: 1;
    min-height: 0;
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
    color: var(--text);
  }
  .row-title { font-weight: 600; font-size: 0.9rem; }
  .row-snippet {
    font-size: 0.75rem;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mono { font-family: var(--font-mono); }

  .editor-pane {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-height: 0;
  }
  .editor-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .editor-head input {
    flex: 1;
    padding: 0.5rem 0.7rem;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-display);
    font-size: 1rem;
    font-weight: 600;
  }
  .editor-head input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .status {
    font-size: 0.75rem;
    color: var(--muted);
    min-width: 5rem;
    text-align: right;
  }
  .status[data-state="saved"]   { color: var(--success); }
  .status[data-state="error"]   { color: var(--danger); }
  .status[data-state="saving"]  { color: var(--accent); }
  .ghost {
    padding: 0.45rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 0.8rem;
  }
  .ghost:hover { color: var(--text); border-color: var(--accent); }
  .ghost.danger:hover { color: var(--danger); border-color: var(--danger); }
  .empty {
    text-align: center;
    padding: 4rem 1rem;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
</style>
