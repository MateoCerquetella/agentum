<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { sessions } from '$stores/sessions';
  import { openPalette } from '$stores/palette';
  import { openNewSession } from '$stores/newSession';
  import { api, type Session, type Status } from '$lib/api';
  import { connStatus } from '$stores/events';
  import {
    profiles,
    activeProfileId,
    setActiveProfile,
    type Profile
  } from '$lib/profiles';
  import { fleet, profileHostHint } from '$stores/fleet';
  import { awaitingInput, idleSessions } from '$stores/attention';

  /**
   * Map server-side Status onto the design's state vocabulary used for
   * the dot color in the sessions list. Priority (highest first):
   *   crashed → `crash`
   *   awaiting input → `attention`
   *   idle at prompt (agent finished turn) → `idle`
   *   running → `live`
   *   else → `idle`
   *
   * Server status stays `running` between turns, so without the
   * awaiting/idle overlays a finished agent reads as a misleading
   * "live" pulsing dot indefinitely. Mirrors the TUI's dot priority
   * (see crates/agentum/src/commands/terminal/ui.rs draw_sessions).
   */
  function stateClass(s: Session, awaiting: Set<string>, idle: Set<string>): string {
    if (s.status === 'crashed') return 'crash';
    if (awaiting.has(s.id)) return 'attention';
    if (idle.has(s.id)) return 'idle';
    if (s.status === 'running') return 'live';
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

  function projectOf(workdir: string | null | undefined): string {
    if (!workdir) return '—';
    const parts = workdir.replace(/\/+$/, '').split('/');
    return parts[parts.length - 1] || workdir || '—';
  }

  // Three-level tree: server → project → session. Mirrors the TUI
  // sidebar the user validated in v0.7.22-era (see auto-memory
  // `feedback-tree-ux`). Each profile becomes a top-level row; its
  // sessions get bucketed by workdir basename. Sessions whose
  // `profile` field is missing (legacy single-endpoint payloads)
  // fall back to the active profile so they don't get orphaned.
  interface ServerNode {
    profile: Profile;
    projects: Array<{ name: string; items: Session[] }>;
    total: number;
    liveCount: number;
  }
  const tree = $derived.by((): ServerNode[] => {
    const byProfile = new Map<string, Session[]>();
    for (const p of $profiles) byProfile.set(p.id, []);
    const activeId = $activeProfileId;
    for (const s of $sessions.items) {
      const pid = s.profile && byProfile.has(s.profile) ? s.profile : activeId;
      const bucket = byProfile.get(pid);
      if (bucket) bucket.push(s);
    }
    return $profiles.map((p) => {
      const items = byProfile.get(p.id) ?? [];
      // Bucket by project, preserving first-seen order so the
      // most-recently-touched project tends to bubble up.
      const order: string[] = [];
      const groups = new Map<string, Session[]>();
      for (const s of items) {
        const k = projectOf(s.workdir);
        if (!groups.has(k)) { groups.set(k, []); order.push(k); }
        groups.get(k)!.push(s);
      }
      for (const k of order) {
        groups.get(k)!.sort((a, b) => {
          const la = a.status === 'running' ? 0 : 1;
          const lb = b.status === 'running' ? 0 : 1;
          return la - lb || a.name.localeCompare(b.name);
        });
      }
      return {
        profile: p,
        projects: order.map((k) => ({ name: k, items: groups.get(k)! })),
        total: items.length,
        liveCount: items.filter((s) => s.status === 'running').length
      };
    });
  });

  /**
   * Match the TUI's `profile_label()` exactly so the dashboard reads
   * the same as `agentum terminal`:
   *   - Loopback (no baseUrl) → real hostname from /api/health
   *     ("omarchy", "mateo-mac"). Falls back to "local" if the probe
   *     hasn't landed yet.
   *   - Named profile → `@<id>` (`@vps`).
   * Single source of truth lives server-side; we just mirror it.
   */
  function serverLabel(p: Profile): string {
    if (!p.baseUrl) {
      const host = $fleet[p.id]?.hostname?.trim();
      return host || 'local';
    }
    return `@${p.id}`;
  }
  function serverHostHint(p: Profile): string {
    return profileHostHint(p);
  }
  function dotClass(p: Profile): string {
    // For the active profile, the events WebSocket is the authoritative
    // real-time signal. The fleet health probe runs every 20 s and can
    // be stale; the WS onclose fires immediately on disconnect, so a
    // reconnecting WS means the server dot must show red *now*, not in
    // up to 20 s when the next fleet probe lands.
    if (p.id === $activeProfileId) {
      if ($connStatus.state === 'connected') return 'live';
      if ($connStatus.state === 'reconnecting') return 'unreachable';
    }
    const e = $fleet[p.id];
    if (!e) return 'unknown';
    return e.status;
  }
  function pickServer(id: string) {
    if (id === $activeProfileId) return;
    setActiveProfile(id);
    if (typeof location !== 'undefined') location.reload();
  }

  // Collapse state, keyed by profile id. Default open; user clicks
  // the chevron to fold. In-memory only — the page reload that fires
  // on profile switch is the natural reset point.
  const collapsed = $state<Record<string, boolean>>({});
  function toggle(id: string) {
    collapsed[id] = !collapsed[id];
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

  <!-- Servers → projects → sessions tree. Mirrors the TUI sidebar
       (see auto-memory feedback-tree-ux). The local profile always
       renders first and labels itself "MY MACHINE (<os>)" so the
       user can scan their fleet by machine. The version chip in the
       section header tracks `/api/health`, so it reflects the
       daemon you're actually talking to — not a stale package.json. -->
  <div class="sect tree-sect">
    <div class="sect-lbl">
      <span>Sessions{serverVersion ? ` · v${serverVersion}` : ''} · {liveCount} live</span>
      <button type="button" class="add" onclick={openNewSession} title="Spawn session" aria-label="Spawn session">+</button>
    </div>
    <div class="tree-scroll">
      {#each tree as node (node.profile.id)}
        {@const p = node.profile}
        {@const isActive = p.id === $activeProfileId}
        {@const isCollapsed = collapsed[p.id] === true}
        {@const status = $fleet[p.id]?.status}
        <div class="server-block" class:active={isActive}>
          <div class="server-row">
            <button
              type="button"
              class="chev"
              onclick={() => toggle(p.id)}
              aria-label={isCollapsed ? 'Expand' : 'Collapse'}
              aria-expanded={!isCollapsed}
            >
              <span class="chev-glyph" class:rot={!isCollapsed}>▸</span>
            </button>
            <button
              type="button"
              class="server-pick"
              onclick={() => pickServer(p.id)}
              title={p.baseUrl || 'current origin'}
            >
              <span class={`srv-dot ${dotClass(p)}`} class:loopback={!p.baseUrl}></span>
              <span class="srv-name">{serverLabel(p)}</span>
              {#if serverHostHint(p)}
                <span class="srv-host">{serverHostHint(p)}</span>
              {/if}
              <span class="srv-count">{node.total}</span>
              {#if isActive}
                <span class="srv-tag">active</span>
              {:else if status === 'unreachable'}
                <span class="srv-tag bad">unreachable</span>
              {:else if status === 'login-needed'}
                <span class="srv-tag warn">login</span>
              {/if}
            </button>
          </div>

          {#if !isCollapsed}
            {#each node.projects as g (g.name)}
              <div class="project-head">
                <span class="proj-name">{g.name}</span>
                <span class="proj-count">{g.items.length}</span>
              </div>
              {#each g.items as s (s.id)}
                <a
                  href={`/sessions/${s.id}`}
                  class="leaf"
                  class:active={s.id === activeSessionId}
                >
                  <span class={`stat ${stateClass(s, $awaitingInput, $idleSessions)}`}></span>
                  <span class="leaf-nm">{s.name}</span>
                  <span class="leaf-tool">{s.tool}</span>
                </a>
              {/each}
            {/each}
            {#if node.projects.length === 0}
              <div class="empty">
                {#if status === 'unreachable'}
                  Unreachable.
                {:else if status === 'login-needed'}
                  Login required.
                {:else}
                  No sessions.
                {/if}
              </div>
            {/if}
          {/if}
        </div>
      {/each}
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

  .tree-sect {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .tree-scroll {
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }

  .server-block {
    display: flex;
    flex-direction: column;
    padding: 2px 0;
  }
  .server-block + .server-block {
    margin-top: 4px;
    border-top: 1px solid var(--border);
    padding-top: 6px;
  }

  /* Server row: chevron + clickable label. Two buttons side-by-side
     so the disclosure toggle doesn't steal the click-to-switch
     gesture from the rest of the row. */
  .server-row {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .chev {
    width: 16px;
    height: 22px;
    display: inline-grid;
    place-items: center;
    background: transparent;
    border: 0;
    color: var(--fg-3);
    cursor: pointer;
    flex: 0 0 auto;
  }
  .chev:hover { color: var(--fg); }
  .chev-glyph {
    display: inline-block;
    font-size: 9px;
    line-height: 1;
    transition: transform 120ms ease;
  }
  .chev-glyph.rot { transform: rotate(90deg); }

  .server-pick {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    padding: 4px 8px 4px 4px;
    background: transparent;
    border: 0;
    border-radius: var(--radius-sm, 4px);
    color: var(--fg);
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition: background var(--t-hover, 150ms ease);
  }
  .server-pick:hover {
    background: color-mix(in srgb, var(--fg) 5%, transparent);
  }
  .server-block.active .server-pick {
    background: color-mix(in srgb, var(--cta) 10%, transparent);
  }
  .srv-dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill, 50%);
    background: var(--fg-3);
    flex: 0 0 auto;
  }
  .srv-dot.live { background: var(--green, #2ea043); }
  .srv-dot.unreachable { background: var(--crash, #ff4d4f); }
  .srv-dot.login-needed { background: var(--warn, #d4a017); }
  .srv-dot.unknown { background: var(--fg-3); }
  .srv-dot.loopback.unknown { background: var(--fg-3); }
  .srv-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    font-family: var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg);
  }
  .srv-host {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 80px;
  }
  .srv-count {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    margin-left: auto;
  }
  .srv-tag {
    font-family: var(--mono);
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--cta);
    margin-left: 4px;
  }
  .srv-tag.bad { color: var(--crash, #ff4d4f); }
  .srv-tag.warn { color: var(--warn, #d4a017); }

  /* Project subgroup header — uppercase, monospaced, slightly
     indented so the tree shape reads at a glance. */
  .project-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 8px 8px 3px 24px;
  }
  .project-head .proj-name {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .project-head .proj-count {
    font-family: var(--mono);
    font-size: 9.5px;
    color: var(--fg-3);
    margin-left: auto;
  }

  /* Session leaf — indented two levels from the server row. Mirrors
     the .sb .item styling but adds the extra indent. */
  .leaf {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 8px 5px 36px;
    border-radius: var(--radius-md);
    color: var(--fg-2);
    font-size: 13px;
    text-decoration: none;
  }
  .leaf:hover { background: var(--bg-row-hover); color: var(--fg); }
  .leaf.active { background: var(--surface); color: var(--fg); }
  .leaf-nm {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .leaf-tool {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
  }
  .leaf .stat {
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill, 50%);
    background: var(--fg-3);
    flex: 0 0 auto;
  }
  .leaf .stat.idle    { background: var(--fg-3); }
  .leaf .stat.compact { background: var(--cta); }
  .leaf .stat.crash   { background: var(--crash); }
  .leaf .stat.live    { background: var(--green, #2ea043); animation: pulse 1.6s infinite; }
  /* Yellow ring for "agent needs you" — distinct from green (live)
     and gray (idle/stopped) so the attention cue reads at a glance. */
  .leaf .stat.attention {
    background: var(--amber, #ffb454);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--amber, #ffb454) 30%, transparent);
  }

  .empty {
    padding: 6px 8px 6px 28px;
    font-size: 11px;
    color: var(--fg-3);
    font-family: var(--mono);
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
    .leaf { padding: 10px 10px 10px 36px; font-size: 14px; }
    .project-head { padding: 10px 8px 4px 24px; }
  }
</style>
