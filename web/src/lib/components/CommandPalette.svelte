<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { palette, closePalette } from '$stores/palette';
  import { openShortcuts } from '$stores/palette';
  import { applyTheme, type Theme } from '$stores/theme';
  import { sessions, loadSessions } from '$stores/sessions';
  import { notes, loadNotes } from '$stores/notes';
  import { board, loadBoard } from '$stores/board';
  import { openNewSession } from '$stores/newSession';

  type Entry = {
    id: string;
    title: string;
    subtitle?: string;
    badge?: string;
    action: () => void;
  };

  let inputEl: HTMLInputElement | null = $state(null);
  let highlight = $state(0);
  let entries = $state<Entry[]>([]);
  let filtered = $state<Entry[]>([]);

  function rebuild() {
    const out: Entry[] = [];

    // Built-in commands
    out.push({ id: 'cmd:theme:terminal-dark', title: 'Switch theme: terminal-dark', badge: 'cmd', action: () => applyTheme('terminal-dark') });
    out.push({ id: 'cmd:theme:paperlight',     title: 'Switch theme: paperlight',     badge: 'cmd', action: () => applyTheme('paperlight') });
    out.push({ id: 'cmd:theme:obsidian-dark',  title: 'Switch theme: obsidian-dark',  badge: 'cmd', action: () => applyTheme('obsidian-dark') });
    out.push({ id: 'cmd:theme:system',          title: 'Switch theme: system',          badge: 'cmd', action: () => applyTheme('system') });
    out.push({ id: 'cmd:shortcuts',             title: 'Show keyboard shortcuts (?)',   badge: 'cmd', action: () => { closePalette(); openShortcuts(); } });
    out.push({ id: 'cmd:new-session',            title: 'New session…',                  badge: 'cmd', subtitle: 'agentum new', action: () => { closePalette(); openNewSession(); } });
    out.push({ id: 'cmd:doctor',                 title: 'Run doctor',                    badge: 'cmd', subtitle: 'agentum doctor', action: () => goto('/doctor') });
    out.push({ id: 'cmd:settings',              title: 'Open settings',                 badge: 'cmd', subtitle: '(soon)', action: () => goto('/settings') });

    // Pages
    const pages: Array<[string, string]> = [
      ['/',         'Sessions'],
      ['/board',    'Board'],
      ['/graph',    'Graph'],
      ['/tools',    'Tools'],
      ['/notes',    'Notes'],
      ['/channels', 'Channels'],
      ['/doctor',   'Doctor'],
      ['/settings', 'Settings'],
    ];
    for (const [href, label] of pages) {
      out.push({ id: `page:${href}`, title: `Go to ${label}`, badge: 'page', action: () => goto(href) });
    }

    // Sessions
    for (const s of $sessions.items) {
      out.push({
        id: `session:${s.id}`,
        title: s.name,
        subtitle: `${s.tool} · ${s.status}`,
        badge: 'session',
        action: () => goto(`/sessions/${s.id}`)
      });
    }

    // Board items (across all columns)
    if ($board.data) {
      for (const col of $board.data.column_order) {
        for (const it of $board.data.columns[col] ?? []) {
          out.push({
            id: `board:${it.id}`,
            title: `${it.key}  ${it.title}`,
            subtitle: `[${it.status}]`,
            badge: 'board',
            action: () => goto('/board')
          });
        }
      }
    }

    // Notes
    for (const n of $notes.items) {
      out.push({
        id: `note:${n.id}`,
        title: n.title || '(untitled)',
        subtitle: n.body.slice(0, 60),
        badge: 'note',
        action: () => goto('/notes')
      });
    }

    entries = out;
    refilter();
  }

  /**
   * Lightweight scoring: prefer prefix matches, then word-boundary, then
   * subsequence. Returns 0 for "no match" so we drop the row.
   */
  function score(item: Entry, q: string): number {
    if (!q) return 1;
    const s = (item.title + ' ' + (item.subtitle ?? '')).toLowerCase();
    const ql = q.toLowerCase();
    if (s.startsWith(ql)) return 100;
    const idx = s.indexOf(ql);
    if (idx === 0) return 80;
    if (idx > 0) {
      const prev = s[idx - 1];
      if (prev === ' ' || prev === '/' || prev === '-' || prev === ':') return 60;
      return 40;
    }
    // Subsequence fallback
    let i = 0;
    for (const c of s) {
      if (c === ql[i]) i += 1;
      if (i >= ql.length) return 10;
    }
    return 0;
  }

  function refilter() {
    const q = $palette.query;
    const ranked = entries
      .map((e) => ({ e, sc: score(e, q) }))
      .filter((x) => x.sc > 0)
      .sort((a, b) => b.sc - a.sc || a.e.title.localeCompare(b.e.title));
    filtered = ranked.slice(0, 50).map((x) => x.e);
    highlight = 0;
  }

  let unsubPalette: (() => void) | null = null;

  onMount(() => {
    unsubPalette = palette.subscribe((s) => {
      if (s.open) {
        // refresh data & entries each open
        loadSessions();
        loadNotes();
        loadBoard();
        rebuild();
        queueMicrotask(() => inputEl?.focus());
      }
    });
  });
  onDestroy(() => unsubPalette?.());

  // Re-filter whenever the query or any source store changes.
  $effect(() => {
    if (!$palette.open) return;
    rebuild();
  });

  function onInput(e: Event) {
    const v = (e.target as HTMLInputElement).value;
    palette.update((s) => ({ ...s, query: v }));
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      closePalette();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlight = Math.min(highlight + 1, filtered.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlight = Math.max(0, highlight - 1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const ent = filtered[highlight];
      if (ent) {
        closePalette();
        ent.action();
      }
    }
  }

  function pick(e: Entry) {
    closePalette();
    e.action();
  }
</script>

{#if $palette.open}
  <div
    class="backdrop"
    role="presentation"
    onclick={closePalette}
    onkeydown={onKeyDown}
    tabindex="-1"
  ></div>
  <div class="palette" role="dialog" aria-label="Command palette">
    <input
      class="search mono"
      type="text"
      placeholder="Type a command, or jump to a session, board item, note…"
      value={$palette.query}
      oninput={onInput}
      onkeydown={onKeyDown}
      bind:this={inputEl}
      autocomplete="off"
      spellcheck="false"
    />
    <div class="list" role="listbox">
      {#if filtered.length === 0}
        <div class="empty muted">no matches</div>
      {/if}
      {#each filtered as e, i (e.id)}
        <button
          type="button"
          class="row"
          class:active={i === highlight}
          onmouseenter={() => (highlight = i)}
          onclick={() => pick(e)}
        >
          {#if e.badge}<span class="badge mono">{e.badge}</span>{/if}
          <span class="title">{e.title}</span>
          {#if e.subtitle}<span class="sub mono">{e.subtitle}</span>{/if}
        </button>
      {/each}
    </div>
    <footer class="hint mono">
      <span>↑↓</span> nav <span>↩</span> open <span>esc</span> close
    </footer>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg) 70%, transparent);
    backdrop-filter: blur(2px);
    z-index: 60;
  }
  .palette {
    position: fixed;
    top: max(8vh, 48px);
    left: 50%;
    transform: translateX(-50%);
    width: min(560px, calc(100% - 2rem));
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 18px 60px rgba(0,0,0,0.35);
    z-index: 61;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    max-height: min(70vh, 600px);
  }
  .search {
    border: 0;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    padding: 0.85rem 1rem;
    font-size: 1rem;
    outline: none;
    font-family: var(--font-mono);
  }
  .list {
    overflow-y: auto;
    padding: 0.3rem;
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.5rem 0.7rem;
    border-radius: 6px;
    text-align: left;
    color: var(--text);
    cursor: pointer;
  }
  .row.active {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .badge {
    font-size: 0.7rem;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    padding: 0.05em 0.45em;
    border-radius: 999px;
    flex-shrink: 0;
  }
  .title {
    flex: 1;
    font-size: 0.9rem;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sub {
    color: var(--muted);
    font-size: 0.75rem;
    flex-shrink: 0;
    max-width: 50%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    padding: 1.25rem;
    text-align: center;
  }
  .hint {
    border-top: 1px solid var(--border);
    padding: 0.5rem 0.9rem;
    color: var(--muted);
    font-size: 0.72rem;
    display: flex;
    gap: 0.7rem;
  }
  .hint span:first-of-type,
  .hint span:not(:last-of-type) {
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 0.35em;
    color: var(--text-2);
  }
  .mono { font-family: var(--font-mono); }
  .muted { color: var(--muted); }
</style>
