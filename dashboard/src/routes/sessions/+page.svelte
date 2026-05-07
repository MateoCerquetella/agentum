<script lang="ts">
  import { onMount } from 'svelte';
  import { sessions, loadSessions } from '$stores/sessions';
  import { openNewSession } from '$stores/newSession';
  import { deriveState, ctxOf } from '$lib/dashboard';
  import FleetRow from '$components/dashboard/FleetRow.svelte';

  type SortKey = 'ctx' | 'tokens' | 'cost' | 'name' | 'state';
  type Filter  = 'all' | 'live' | 'incidents';

  let sortKey: SortKey = $state('ctx');
  let sortDir: 'asc' | 'desc' = $state('asc');
  let filter: Filter = $state('all');
  let query = $state('');

  function refresh() { loadSessions(); }
  onMount(() => {
    refresh();
    const id = setInterval(refresh, 5000);
    return () => clearInterval(id);
  });

  // Project key: last meaningful segment of workdir, fallback to "—".
  // ~/Developer/projects/agentum  →  agentum
  // /opt/work/foo/                 →  foo
  function projectOf(workdir: string | null | undefined): string {
    if (!workdir) return '—';
    const parts = workdir.replace(/\/+$/, '').split('/');
    const tail = parts[parts.length - 1] || workdir;
    return tail || '—';
  }

  const filtered = $derived.by(() => {
    let xs = $sessions.items.slice();
    if (filter === 'live')      xs = xs.filter(s => deriveState(s) === 'live');
    if (filter === 'incidents') xs = xs.filter(s => {
      const st = deriveState(s); return st === 'crash' || st === 'compact';
    });
    const q = query.trim().toLowerCase();
    if (q) xs = xs.filter(s =>
      s.name.toLowerCase().includes(q) ||
      (s.tool ?? '').toLowerCase().includes(q) ||
      (s.workdir ?? '').toLowerCase().includes(q)
    );
    const dir = sortDir === 'asc' ? 1 : -1;
    xs.sort((a, b) => {
      switch (sortKey) {
        case 'ctx':    return (ctxOf(a) - ctxOf(b)) * dir;
        case 'tokens': return ((a.tokens ?? 0) - (b.tokens ?? 0)) * dir;
        case 'cost':   return ((a.cost ?? 0) - (b.cost ?? 0)) * dir;
        case 'name':   return a.name.localeCompare(b.name) * dir;
        case 'state':  return deriveState(a).localeCompare(deriveState(b)) * dir;
      }
    });
    return xs;
  });

  // Group filtered list by project. Order: groups appear in the order
  // their first member appears in `filtered`, so the active sort still
  // controls priority across the whole page.
  const grouped = $derived.by(() => {
    const order: string[] = [];
    const map = new Map<string, typeof filtered>();
    for (const s of filtered) {
      const key = projectOf(s.workdir);
      if (!map.has(key)) { map.set(key, []); order.push(key); }
      map.get(key)!.push(s);
    }
    return order.map(k => ({ project: k, items: map.get(k)! }));
  });

  function toggleSort(k: SortKey) {
    if (sortKey === k) sortDir = sortDir === 'asc' ? 'desc' : 'asc';
    else { sortKey = k; sortDir = 'asc'; }
  }
  function sortMark(k: SortKey): string {
    if (sortKey !== k) return '';
    return sortDir === 'asc' ? '↑' : '↓';
  }
</script>

<div class="page">
  <div class="toolbar">
    <span class="micro" style="color: var(--fg-2);">Sessions</span>
    <span class="micro" style="margin-left: 4px;">· {filtered.length} of {$sessions.items.length}</span>
    <span class="spacer"></span>
    <div class="filters">
      <button type="button" class="seg" class:on={filter === 'all'}       onclick={() => filter = 'all'}>All</button>
      <button type="button" class="seg" class:on={filter === 'live'}      onclick={() => filter = 'live'}>Live</button>
      <button type="button" class="seg" class:on={filter === 'incidents'} onclick={() => filter = 'incidents'}>Incidents</button>
    </div>
    <input
      class="search"
      type="search"
      placeholder="filter by name / tool / path"
      bind:value={query}
      autocomplete="off"
      spellcheck="false"
    />
    <button type="button" class="tb-btn primary" onclick={openNewSession}>
      <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
        <path d="M3 8h10M8 3v10" stroke-linecap="round"/>
      </svg>
      Spawn session
    </button>
  </div>

  <div class="scroll">
    {#if $sessions.loading && $sessions.items.length === 0}
      <div class="empty mono">Loading sessions…</div>
    {:else if $sessions.error}
      <div class="empty mono err">Failed to load: {$sessions.error}</div>
    {:else if $sessions.items.length === 0}
      <div class="empty mono">
        No sessions yet — <button type="button" class="link" onclick={openNewSession}>spawn one</button>.
      </div>
    {:else}
      <div class="fleet">
        <div class="fleet-head">
          <span></span>
          <button type="button" class="hcell" onclick={() => toggleSort('name')}>Session · task <span class="mark">{sortMark('name')}</span></button>
          <span class="hcell">Last activity</span>
          <button type="button" class="hcell right" onclick={() => toggleSort('tokens')}>Tokens <span class="mark">{sortMark('tokens')}</span></button>
          <button type="button" class="hcell right" onclick={() => toggleSort('cost')}>Cost <span class="mark">{sortMark('cost')}</span></button>
          <button type="button" class="hcell right" onclick={() => toggleSort('ctx')}>Context <span class="mark">{sortMark('ctx')}</span></button>
          <span></span>
        </div>

        {#each grouped as g (g.project)}
          <div class="group-head">
            <span class="g-name">{g.project}</span>
            <span class="g-count">{g.items.length}</span>
          </div>
          {#each g.items as s (s.id)}
            <FleetRow {s} />
          {/each}
        {/each}

        {#if filtered.length === 0}
          <div class="empty mono">
            No sessions match <code>{query || filter}</code>.
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .page {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 16px;
  }
  .spacer { flex: 1; }

  .filters {
    display: inline-flex;
    gap: 0;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .seg {
    background: transparent;
    border: 0;
    color: var(--fg-3);
    font-family: var(--mono);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 5px 12px;
    cursor: pointer;
    transition: color var(--t-hover), background var(--t-hover);
  }
  .seg:hover { color: var(--fg-2); }
  .seg.on { color: var(--fg); background: var(--surface-2); }

  .search {
    width: 220px;
    padding: 5px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 12px;
  }
  .search:focus {
    outline: none;
    border-color: var(--link);
  }
  .search::placeholder { color: var(--fg-3); }

  .fleet {
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: 10px;
    overflow: hidden;
  }
  .fleet-head {
    display: grid;
    grid-template-columns: 14px 1.6fr 1.2fr 90px 80px 96px 80px;
    gap: 14px;
    padding: 9px 16px;
    background: #0a0a0a;
    border-bottom: 1px solid var(--border);
  }
  .hcell {
    background: transparent;
    border: 0;
    color: var(--fg-3);
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    cursor: pointer;
    padding: 0;
    text-align: left;
    transition: color var(--t-hover);
  }
  .hcell:hover { color: var(--fg-2); }
  .hcell.right { text-align: right; }
  .mark { color: var(--fg-2); }

  .group-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 10px 16px 6px;
    background: #0c0c0c;
    border-bottom: 1px solid var(--border);
  }
  .g-name {
    font-family: var(--mono);
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-2);
  }
  .g-count {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
  }

  .empty {
    padding: 28px 16px;
    color: var(--fg-3);
    font-size: 12px;
    text-align: center;
  }
  .empty.err { color: var(--crash); }
  .empty .link {
    background: transparent;
    border: 0;
    color: var(--link);
    text-decoration: underline;
    font: inherit;
    cursor: pointer;
  }
  .empty code {
    font-family: var(--mono);
    color: var(--fg-2);
    background: var(--bg-2);
    padding: 1px 5px;
    border-radius: 3px;
  }

  /* Hide the desktop column header on tablets — FleetRow already
     collapses there. The first FleetRow is self-explanatory thanks to
     the project label, context bar, and Open button. */
  @media (max-width: 1100px) {
    .fleet-head { display: none; }
  }

  @media (max-width: 720px) {
    .scroll { padding: 12px; }
    .search {
      width: 100%;
      min-width: 0;
      flex: 1 1 100%;
      order: 3;
      padding: 10px 12px;
      border-radius: 10px;
    }
    /* Hide the duplicate spawn button on phone — bottom-nav FAB owns
       the primary spawn action. */
    :global(.toolbar .tb-btn.primary) { display: none; }
    /* Sticky route header so filters stay reachable while scrolling. */
    :global(.toolbar) {
      position: sticky;
      top: 0;
      z-index: 5;
      background: color-mix(in srgb, var(--bg-chrome) 92%, transparent);
      backdrop-filter: blur(10px);
      -webkit-backdrop-filter: blur(10px);
      flex-wrap: wrap;
      gap: 8px;
    }
    .filters {
      flex: 1 1 100%;
      order: 2;
      width: 100%;
    }
    .seg {
      flex: 1;
      padding: 9px 8px;
      font-size: 11px;
      min-height: 36px;
    }
    .fleet { border-radius: 12px; }
  }
</style>
