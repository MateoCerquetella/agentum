<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type DirEntry } from '$lib/api';

  type Props = {
    value: string;
    onChange: (next: string) => void;
    placeholder?: string;
    required?: boolean;
  };
  let { value = $bindable(), onChange, placeholder = '/home/you/projects/foo', required = false }: Props = $props();

  let input: HTMLInputElement | null = $state(null);
  let open = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let listed = $state<DirEntry[]>([]);
  let listedFor = $state<string>('');
  let highlight = $state(-1);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  // Whatever the user has typed; we read this each open and after each
  // refresh to drive the displayed dropdown.
  function parentAndPrefix(input: string): { parent: string; prefix: string } {
    if (!input) return { parent: '~', prefix: '' };
    if (input === '~') return { parent: '~', prefix: '' };
    if (input === '/') return { parent: '/', prefix: '' };

    const trimmedRight = input.endsWith('/') ? input.slice(0, -1) : input;
    const lastSlash = trimmedRight.lastIndexOf('/');
    if (lastSlash < 0) {
      // no slash yet — treat as prefix under $HOME
      return { parent: '~', prefix: trimmedRight };
    }
    if (input.endsWith('/')) {
      return { parent: trimmedRight || '/', prefix: '' };
    }
    const parent = trimmedRight.slice(0, lastSlash) || '/';
    const prefix = trimmedRight.slice(lastSlash + 1);
    return { parent, prefix };
  }

  async function refresh() {
    const { parent } = parentAndPrefix(value);
    if (parent === listedFor && listed.length) return; // cached
    loading = true;
    error = null;
    try {
      const r = await api.listDir(parent || undefined);
      listed = r.dirs;
      listedFor = parent;
      highlight = -1;
    } catch (e) {
      listed = [];
      listedFor = parent;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  let visible = $derived.by(() => {
    const { prefix } = parentAndPrefix(value);
    if (!prefix) return listed.slice(0, 50);
    const lp = prefix.toLowerCase();
    return listed
      .filter((d) => d.name.toLowerCase().includes(lp))
      .sort((a, b) => {
        // prefix matches rank above mid-string matches
        const ai = a.name.toLowerCase().startsWith(lp) ? 0 : 1;
        const bi = b.name.toLowerCase().startsWith(lp) ? 0 : 1;
        if (ai !== bi) return ai - bi;
        return a.name.localeCompare(b.name);
      })
      .slice(0, 50);
  });

  function scheduleRefresh() {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(refresh, 80);
  }

  function onInput() {
    onChange(value);
    open = true;
    scheduleRefresh();
  }

  function pick(entry: DirEntry) {
    value = entry.path.endsWith('/') ? entry.path : entry.path + '/';
    onChange(value);
    listedFor = ''; // force refetch under the new dir
    refresh();
    input?.focus();
  }

  function commit(entry: DirEntry) {
    value = entry.path;
    onChange(value);
    open = false;
  }

  function onKey(e: KeyboardEvent) {
    if (!open) {
      if (e.key === 'ArrowDown') {
        open = true;
        refresh();
        e.preventDefault();
      }
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlight = Math.min(highlight + 1, visible.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlight = Math.max(-1, highlight - 1);
    } else if (e.key === 'Enter') {
      if (highlight >= 0 && visible[highlight]) {
        e.preventDefault();
        pick(visible[highlight]);
      }
      // else let the form submit
    } else if (e.key === 'Tab' && visible[0]) {
      e.preventDefault();
      pick(visible[highlight >= 0 ? highlight : 0]);
    } else if (e.key === 'Escape') {
      open = false;
    }
  }

  function onFocus() {
    open = true;
    refresh();
  }

  function onBlur(e: FocusEvent) {
    // Don't close if focus moved to a dropdown item.
    const next = e.relatedTarget as HTMLElement | null;
    if (next && next.closest('.dir-dropdown')) return;
    setTimeout(() => (open = false), 80);
  }

  onMount(() => {
    // Prime the listing so the dropdown isn't empty on first focus.
    refresh();
  });
</script>

<div class="wrap">
  <input
    bind:this={input}
    bind:value
    type="text"
    {placeholder}
    {required}
    autocomplete="off"
    spellcheck="false"
    oninput={onInput}
    onkeydown={onKey}
    onfocus={onFocus}
    onblur={onBlur}
  />
  {#if open}
    <div class="dir-dropdown" tabindex="-1">
      {#if loading}
        <div class="row muted">listing {listedFor}…</div>
      {:else if error}
        <div class="row err">{error}</div>
      {:else if visible.length === 0}
        <div class="row muted">no matching directories in {listedFor}</div>
      {:else}
        <div class="row meta">
          <span class="muted">in</span>
          <span class="path mono">{listedFor || '~'}</span>
        </div>
        {#each visible as d, i (d.path)}
          <button
            type="button"
            class="row item"
            class:active={i === highlight}
            onmouseenter={() => (highlight = i)}
            onmousedown={(e) => { e.preventDefault(); pick(d); }}
            ondblclick={() => commit(d)}
            title={d.path}
          >
            <span class="caret" aria-hidden="true">›</span>
            <span class="name mono">{d.name}</span>
          </button>
        {/each}
      {/if}
      <div class="row hint mono">
        <span>↩ enter dir</span>
        <span>tab autocomplete</span>
        <span>dbl-click pick</span>
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap { position: relative; }
  input {
    width: 100%;
    padding: 0.5rem 0.7rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.85rem;
    box-sizing: border-box;
  }
  input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }

  .dir-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    max-height: 18rem;
    overflow-y: auto;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    z-index: 100;
    padding: 0.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.05rem;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.35rem 0.55rem;
    border: 0;
    background: transparent;
    color: var(--text-2);
    text-align: left;
    font-size: 0.8rem;
    border-radius: 4px;
    cursor: pointer;
  }
  .row.muted { color: var(--muted); cursor: default; }
  .row.err { color: var(--danger); cursor: default; }
  .row.meta { cursor: default; padding-bottom: 0.25rem; border-bottom: 1px solid var(--border); margin-bottom: 0.15rem; }
  .row.meta .path { color: var(--accent); }
  .row.item:hover, .row.item.active {
    background: color-mix(in srgb, var(--accent) 12%, var(--surface));
    color: var(--text);
  }
  .caret { color: var(--muted); font-family: var(--font-mono); }
  .name { font-family: var(--font-mono); font-size: 0.82rem; }
  .mono { font-family: var(--font-mono); }
  .hint {
    color: var(--muted);
    font-size: 0.7rem;
    border-top: 1px solid var(--border);
    margin-top: 0.15rem;
    padding-top: 0.4rem;
    cursor: default;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
</style>
