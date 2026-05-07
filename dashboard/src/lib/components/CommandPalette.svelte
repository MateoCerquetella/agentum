<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { goto } from '$app/navigation';
  import { palette, closePalette } from '$stores/palette';
  import { openShortcuts } from '$stores/palette';
  import { sessions, loadSessions } from '$stores/sessions';
  import { openNewSession } from '$stores/newSession';
  import { tweaks, setTheme } from '$stores/tweaks';
  import { THEMES } from '$stores/themes';
  import { api } from '$lib/api';

  type Entry = {
    id: string;
    title: string;
    subtitle?: string;
    badge?: string;
    /** Optional grouping label that renders as a non-selectable header
     *  immediately before this entry. Used for the Dark / Light split
     *  in the theme commands. */
    section?: string;
    /** Hex swatch shown to the left of the title. */
    swatch?: string;
    /** Marks the row with a check; used for the active theme. */
    selected?: boolean;
    action: () => void;
  };

  let inputEl: HTMLInputElement | null = $state(null);
  let highlight = $state(0);
  let entries = $state<Entry[]>([]);
  let filtered = $state<Entry[]>([]);

  function rebuild() {
    const out: Entry[] = [];

    // Built-in commands
    out.push({ id: 'cmd:shortcuts',             title: 'Show keyboard shortcuts (?)',   badge: 'cmd', action: () => { closePalette(); openShortcuts(); } });
    out.push({ id: 'cmd:new-session',            title: 'New agent…',                    badge: 'cmd', subtitle: 'agentum new', action: () => { closePalette(); openNewSession(); } });
    out.push({ id: 'cmd:spawn-shell',            title: 'Spawn plain shell (bash)',      badge: 'cmd', subtitle: 'TUI parity: t', action: () => { closePalette(); spawnShellFromPalette(); } });
    out.push({ id: 'cmd:settings',               title: 'Open settings',                 badge: 'cmd', action: () => goto('/settings') });

    // Pages
    const pages: Array<[string, string]> = [
      ['/',         'Agents'],
      ['/settings', 'Settings'],
    ];
    for (const [href, label] of pages) {
      out.push({ id: `page:${href}`, title: `Go to ${label}`, badge: 'page', action: () => goto(href) });
    }

    // Themes — grouped under Dark / Light section headers. The first
    // entry in each group carries a `section` label that renders as a
    // non-selectable divider immediately above it.
    const activeId = get(tweaks).theme;
    const dark  = THEMES.filter(t => t.mode === 'dark');
    const light = THEMES.filter(t => t.mode === 'light');
    for (const t of dark) {
      out.push({
        id: `theme:${t.id}`,
        title: `Theme · ${t.label}`,
        badge: 'theme',
        section: 'Dark themes',
        swatch: t.swatch,
        selected: t.id === activeId,
        action: () => { closePalette(); setTheme(t.id); }
      });
    }
    for (const t of light) {
      out.push({
        id: `theme:${t.id}`,
        title: `Theme · ${t.label}`,
        badge: 'theme',
        section: 'Light themes',
        swatch: t.swatch,
        selected: t.id === activeId,
        action: () => { closePalette(); setTheme(t.id); }
      });
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

    entries = out;
    refilter();
  }

  /**
   * Lightweight scoring: prefer prefix matches, then word-boundary, then
   * subsequence. Returns 0 for "no match" so we drop the row.
   */
  function score(item: Entry, q: string): number {
    if (!q) return 1;
    const s = (item.title + ' ' + (item.subtitle ?? '') + ' ' + (item.badge ?? '') + ' ' + (item.section ?? '')).toLowerCase();
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

  // Mirrors the `t` shortcut in the TUI: create a `bash` session in the
  // user's home, start it, and navigate to its detail page. Errors fall
  // through to the console — the dashboard's main page has its own banner;
  // here we keep the palette interaction silent on success.
  async function spawnShellFromPalette() {
    try {
      let workdir = '.';
      try {
        const home = await api.listDir();
        if (home?.path) workdir = home.path;
      } catch { /* best-effort */ }
      const suffix = Math.random().toString(16).slice(2, 8);
      const created = await api.createSession({
        name: `shell-${suffix}`,
        workdir,
        tool: 'bash',
        model: null,
        flags: []
      });
      try { await api.startSession(created.id); } catch { /* surfaced on detail page */ }
      await loadSessions();
      await goto(`/sessions/${created.id}`);
    } catch (err) {
      console.error('spawn-shell failed', err);
    }
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
        {#if e.section && (i === 0 || filtered[i - 1].section !== e.section)}
          <div class="section mono" role="presentation">{e.section}</div>
        {/if}
        <button
          type="button"
          class="row"
          class:active={i === highlight}
          onmouseenter={() => (highlight = i)}
          onclick={() => pick(e)}
        >
          {#if e.swatch}
            <span class="swatch" style:background={e.swatch}></span>
          {:else if e.badge}
            <span class="badge mono">{e.badge}</span>
          {/if}
          <span class="title">{e.title}</span>
          {#if e.selected}
            <svg class="check" width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3.5 8l3 3 6-6"/>
            </svg>
          {/if}
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
  /* Theme rows: small color disc instead of the badge so users can
     eyeball the palette before committing to it. */
  .swatch {
    width: 14px;
    height: 14px;
    border-radius: 999px;
    flex-shrink: 0;
    border: 1px solid color-mix(in srgb, var(--text) 18%, transparent);
  }
  .check {
    color: var(--accent);
    flex-shrink: 0;
    margin-left: 0.4rem;
  }
  /* Non-selectable section divider; sits between filtered theme groups. */
  .section {
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    padding: 0.6rem 0.7rem 0.25rem;
    user-select: none;
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

  /* Phone: command palette becomes a near-fullscreen sheet that
     surfaces the soft keyboard immediately. Hint footer collapses. */
  @media (max-width: 720px) {
    .palette {
      top: env(safe-area-inset-top, 0px);
      bottom: env(safe-area-inset-bottom, 0px);
      left: 0;
      right: 0;
      width: 100%;
      max-width: 100%;
      max-height: none;
      transform: none;
      border-radius: 0;
      border-left: 0;
      border-right: 0;
    }
    .search {
      padding: 14px 16px;
      font-size: 16px;
    }
    .list { padding: 8px; gap: 4px; }
    .row { padding: 12px; min-height: 44px; }
    .hint { display: none; }
  }
</style>
