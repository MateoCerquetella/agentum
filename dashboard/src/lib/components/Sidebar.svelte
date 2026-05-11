<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { sessions } from '$stores/sessions';
  import { openPalette } from '$stores/palette';
  import { openNewSession } from '$stores/newSession';
  import { api, type Status } from '$lib/api';
  import {
    profiles,
    activeProfileId,
    setActiveProfile,
    type Profile
  } from '$lib/profiles';

  /**
   * Map server-side Status onto the design's state vocabulary used for
   * the dot color in the sessions list. `compact` is derived from the
   * server when ctx data lands; until then `running` maps to `live`.
   */
  function stateClass(status: Status): string {
    if (status === 'running') return 'live';
    if (status === 'crashed') return 'crash';
    return 'idle';
  }

  const activeSessionId = $derived.by(() => {
    const m = page.url.pathname.match(/^\/sessions\/([^/]+)/);
    return m ? m[1] : null;
  });

  const activeView = $derived.by(() => {
    const p = page.url.pathname;
    if (p === '/') return 'overview';
    if (p.startsWith('/board')) return 'board';
    if (p.startsWith('/sessions')) return 'sessions';
    if (p.startsWith('/terminals')) return 'terminals';
    if (p.startsWith('/settings')) return 'settings';
    return '';
  });

  const liveCount = $derived($sessions.items.filter(s => s.status === 'running').length);

  // Version chip pulls from /api/health so it tracks the actual
  // `agentum serve` we're talking to — not dashboard/package.json
  // (which has been stuck at 0.1.0 since project init).
  let serverVersion = $state<string | null>(null);
  onMount(() => {
    let cancelled = false;
    api.health()
      .then(h => { if (!cancelled) serverVersion = h.version; })
      .catch(() => {});
    return () => { cancelled = true; };
  });

  // Group the sidebar sessions by project (basename of `workdir`).
  // Live first within each group, then alphabetical. Group order
  // reflects first-encounter order so the most-recently-touched
  // project tends to bubble up.
  function projectOf(workdir: string | null | undefined): string {
    if (!workdir) return '—';
    const parts = workdir.replace(/\/+$/, '').split('/');
    return parts[parts.length - 1] || workdir || '—';
  }
  const groups = $derived.by(() => {
    const order: string[] = [];
    const map = new Map<string, typeof $sessions.items>();
    for (const s of $sessions.items) {
      const k = projectOf(s.workdir);
      if (!map.has(k)) { map.set(k, []); order.push(k); }
      map.get(k)!.push(s);
    }
    for (const k of order) {
      map.get(k)!.sort((a, b) => {
        const la = a.status === 'running' ? 0 : 1;
        const lb = b.status === 'running' ? 0 : 1;
        return la - lb || a.name.localeCompare(b.name);
      });
    }
    return order.map(k => ({ project: k, items: map.get(k)! }));
  });

  // Mirror the TUI's SERVERS sidebar section: list every configured
  // profile, label loopback (empty baseUrl) as "this machine", and
  // make the active profile read at a glance. Clicking a server
  // switches the dashboard's active endpoint via the same reload
  // path the topbar EndpointSwitcher uses — keeps every store, WS,
  // and cache coherent with the new origin without per-store
  // re-init logic.
  function serverLabel(p: Profile): string {
    return p.baseUrl ? p.label : 'this machine';
  }
  function pickServer(id: string) {
    if (id === $activeProfileId) return;
    setActiveProfile(id);
    if (typeof location !== 'undefined') location.reload();
  }
</script>

<aside class="sb">
  <!-- Full-width search — replaces the workspace card chrome which had
       no functional behavior. -->
  <div class="ws-switcher">
    <button type="button" class="ws-search" onclick={openPalette} aria-label="Open command palette">
      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
        <circle cx="7" cy="7" r="4.5"/>
        <path d="M10.5 10.5L13.5 13.5"/>
      </svg>
      <span class="lbl">Search sessions, commands…</span>
      <span class="kbd">⌘K</span>
    </button>
  </div>

  <!-- Top-level nav -->
  <div class="sect">
    <a href="/" class="item" class:active={activeView === 'overview'}>
      <svg class="ico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
        <rect x="2" y="2" width="5" height="5" rx="1"/>
        <rect x="9" y="2" width="5" height="5" rx="1"/>
        <rect x="2" y="9" width="5" height="5" rx="1"/>
        <rect x="9" y="9" width="5" height="5" rx="1"/>
      </svg>
      <span class="nm">Overview</span>
    </a>
    <a href="/sessions" class="item" class:active={activeView === 'sessions'}>
      <svg class="ico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
        <rect x="2" y="3" width="12" height="10" rx="1.5"/>
        <path d="M2 6h12"/>
      </svg>
      <span class="nm">Sessions</span>
      <span class="count">{$sessions.items.length}</span>
    </a>
    <a href="/board" class="item" class:active={activeView === 'board'}>
      <svg class="ico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
        <rect x="2" y="2" width="3.5" height="12" rx="0.6"/>
        <rect x="6.5" y="2" width="3" height="8" rx="0.6"/>
        <rect x="10.5" y="2" width="3.5" height="6" rx="0.6"/>
      </svg>
      <span class="nm">Board</span>
    </a>
    <a href="/terminals" class="item" class:active={activeView === 'terminals'}>
      <svg class="ico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
        <rect x="2" y="3" width="12" height="10" rx="1.5"/>
        <path d="M5 7l2 1.5L5 10" stroke-linecap="round" stroke-linejoin="round"/>
        <path d="M9 10h2" stroke-linecap="round"/>
      </svg>
      <span class="nm">Terminals</span>
    </a>
    <a href="/settings" class="item" class:active={activeView === 'settings'}>
      <svg class="ico" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4">
        <circle cx="8" cy="8" r="2.2"/>
        <path d="M8 1.5v2M8 12.5v2M14.5 8h-2M3.5 8h-2M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4M12.6 12.6l-1.4-1.4M4.8 4.8L3.4 3.4" stroke-linecap="round"/>
      </svg>
      <span class="nm">Settings</span>
    </a>
  </div>

  <!-- Servers section: mirrors the TUI sidebar. Always shows the
       full profile list — loopback included — so the user can see
       "this machine" sitting alongside any configured peers without
       having to open the topbar dropdown. -->
  <div class="sect servers-sect">
    <div class="sect-lbl">
      <span>Servers</span>
    </div>
    {#each $profiles as p (p.id)}
      <button
        type="button"
        class="server-row"
        class:active={p.id === $activeProfileId}
        onclick={() => pickServer(p.id)}
        title={p.baseUrl || 'current origin'}
      >
        <span class="srv-dot" class:loopback={!p.baseUrl}></span>
        <span class="srv-name">{serverLabel(p)}</span>
        {#if p.id === $activeProfileId}
          <span class="srv-tag">active</span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- Sessions list (live updating, grouped by project) -->
  <div class="sect sessions-sect">
    <div class="sect-lbl">
      <span>Sessions · {liveCount} live</span>
      <button type="button" class="add" onclick={openNewSession} title="Spawn session" aria-label="Spawn session">+</button>
    </div>
    <div class="sessions-scroll">
      {#each groups as g (g.project)}
        <div class="group-head">
          <span class="g-name">{g.project}</span>
          <span class="g-count">{g.items.length}</span>
        </div>
        {#each g.items as s (s.id)}
          <a
            href={`/sessions/${s.id}`}
            class="item"
            class:active={s.id === activeSessionId}
          >
            <span class={`stat ${stateClass(s.status)}`}></span>
            <span class="nm">{s.name}</span>
            <span class="count">{s.tool}</span>
          </a>
        {/each}
      {/each}
      {#if $sessions.items.length === 0}
        <div class="empty">No sessions yet.</div>
      {/if}
    </div>
  </div>

  <div class="footer">
    <span class="dot"></span>
    <span style="color: var(--fg-2);">tmux</span>
    <span>· {liveCount} pane{liveCount === 1 ? '' : 's'}</span>
    <span style="flex: 1;"></span>
    <span style="color: var(--fg-2);">{serverVersion ? `v${serverVersion}` : ''}</span>
  </div>
</aside>

<style>
  /* Most styles come from .sb in _design.css. Locals here cover only
     overrides that depend on this component's structure. */

  /* Override _design.css: drop the inset on the switcher container so
     the search bar sits flush, then upsize the search itself into a
     prominent full-width primary control. */
  :global(.sb .ws-switcher) {
    padding: 10px 10px;
  }
  :global(.sb .ws-search) {
    width: 100%;
    margin-top: 0;
    padding: 8px 10px;
    font-size: 12.5px;
  }
  :global(.sb .ws-search:hover) {
    color: var(--fg-2);
    border-color: var(--fg-3);
  }

  .servers-sect {
    /* Stays at a fixed height so the sessions list below still owns
       the scroll. Caps with a short max-height + internal overflow so
       a user with many servers doesn't lose the sessions area. */
    flex: 0 0 auto;
    max-height: 30vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .server-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    background: transparent;
    border: 0;
    border-radius: var(--radius-sm, 4px);
    color: var(--fg-2);
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition: background var(--t-hover, 150ms ease), color var(--t-hover, 150ms ease);
  }
  .server-row:hover {
    background: color-mix(in srgb, var(--fg) 5%, transparent);
    color: var(--fg);
  }
  .server-row.active {
    background: color-mix(in srgb, var(--cta) 12%, transparent);
    color: var(--fg);
  }
  .srv-dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill, 50%);
    background: var(--cta);
    flex: 0 0 auto;
  }
  /* "this machine" / loopback gets a different glyph so the user
     reads it as a special row even before checking the label. */
  .srv-dot.loopback {
    background: var(--green, #2ea043);
  }
  .srv-name {
    flex: 1;
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .srv-tag {
    font-family: var(--mono);
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--cta);
  }

  .sessions-sect {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .sessions-scroll {
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }
  .empty {
    padding: 6px 8px;
    font-size: 12px;
    color: var(--fg-3);
    font-family: var(--mono);
  }

  /* Project group header within the sessions list. */
  .group-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 10px 8px 4px;
  }
  .group-head .g-name {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group-head .g-count {
    font-family: var(--mono);
    font-size: 9.5px;
    color: var(--fg-3);
    margin-left: auto;
  }

  /* Phone: drawer-mode sidebar gets fatter rows so taps actually land
     and the search bar reads as the dominant action. */
  @media (max-width: 720px) {
    :global(.sb .ws-switcher) { padding: 14px 12px 10px; }
    :global(.sb .ws-search) {
      padding: 11px 12px;
      font-size: 14px;
      border-radius: 10px;
    }
    :global(.sb .item) {
      padding: 12px 10px;
      font-size: 15px;
      border-radius: 10px;
    }
    :global(.sb .item .ico) { width: 18px; height: 18px; }
    :global(.sb .sect) { padding: 10px 12px; }
    :global(.sb .footer) {
      padding: 12px;
      font-size: 12px;
    }
  }
</style>
