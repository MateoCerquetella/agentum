<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { sessions } from '$stores/sessions';
  import { openPalette } from '$stores/palette';
  import { openNewSession } from '$stores/newSession';
  import type { Status } from '$lib/api';

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
</script>

<aside class="sb">
  <!-- Workspace switcher -->
  <div class="ws-switcher">
    <button type="button" class="ws-card" aria-label="Switch workspace">
      <span class="glyph">A</span>
      <span class="meta">
        <span class="name">Agentum</span>
        <span class="host">localhost · 8822</span>
      </span>
      <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" style="color: var(--fg-3); flex-shrink: 0;">
        <path d="M5 6l3 3 3-3M5 10l3 3 3-3"/>
      </svg>
    </button>
    <button type="button" class="ws-search" onclick={openPalette}>
      <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
        <circle cx="7" cy="7" r="4.5"/>
        <path d="M10.5 10.5L13.5 13.5"/>
      </svg>
      <span class="lbl">Search</span>
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

  <!-- Sessions list (live updating) -->
  <div class="sect sessions-sect">
    <div class="sect-lbl">
      <span>Sessions · {liveCount} live</span>
      <button type="button" class="add" onclick={openNewSession} title="Spawn session" aria-label="Spawn session">+</button>
    </div>
    <div class="sessions-scroll">
      {#each $sessions.items as s (s.id)}
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
    <span style="color: var(--fg-2);">v{__APP_VERSION__}</span>
  </div>
</aside>

<style>
  /* Most styles come from .sb in _design.css. Locals here cover only
     overrides that depend on this component's structure. */
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
</style>
