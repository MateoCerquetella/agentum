<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { sessions, loadSessions } from '$stores/sessions';
  import { openNewSession } from '$stores/newSession';
  import SessionCard from '$components/SessionCard.svelte';
  import EmptyState from '$components/EmptyState.svelte';
  import Skeleton from '$components/Skeleton.svelte';
  import { api, type Health } from '$lib/api';

  let runningOnly = $state(false);
  let health = $state<Health | null>(null);
  let spawningShell = $state(false);
  let spawnError = $state<string | null>(null);

  function refresh() {
    loadSessions(runningOnly ? 'running' : undefined);
  }

  async function loadHealth() {
    try { health = await api.health(); } catch { /* non-critical */ }
  }

  onMount(() => {
    refresh();
    loadHealth();
    const id = setInterval(() => { refresh(); loadHealth(); }, 5000);
    return () => clearInterval(id);
  });

  function toggleRunning() {
    runningOnly = !runningOnly;
    refresh();
  }

  let stats = $derived.by(() => {
    const items = $sessions.items;
    return {
      total: items.length,
      running: items.filter(s => s.status === 'running').length,
      stopped: items.filter(s => s.status === 'stopped').length,
      crashed: items.filter(s => s.status === 'crashed').length,
      tools: [...new Set(items.map(s => s.tool))].length
    };
  });

  let recentActivity = $derived.by(() => {
    return [...$sessions.items]
      .filter(s => s.last_activity_at)
      .sort((a, b) => new Date(b.last_activity_at!).getTime() - new Date(a.last_activity_at!).getTime())
      .slice(0, 5);
  });

  function fmtRel(ts: string | null): string {
    if (!ts) return '—';
    const d = new Date(ts);
    const diff = (Date.now() - d.getTime()) / 1000;
    if (diff < 60) return `${Math.floor(diff)}s ago`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return d.toLocaleDateString();
  }

  function fmtUptime(s: number): string {
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.floor(s / 60)}m`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    return `${Math.floor(s / 86400)}d`;
  }

  // One-click "spawn plain terminal" — mirrors the TUI's `t` shortcut.
  // Picks the next free `shell-N` name (sequential, scanning current
  // sessions), asks the server for a default workdir (the user's home),
  // creates a `bash` session, starts it, and navigates to the detail
  // page so the user lands on the live pane.
  function nextShellName(): string {
    let max = 0;
    for (const s of $sessions.items) {
      const m = /^shell-(\d+)$/.exec(s.name);
      if (m) {
        const n = parseInt(m[1], 10);
        if (Number.isFinite(n) && n > max) max = n;
      }
    }
    return `shell-${max + 1}`;
  }

  async function spawnShell() {
    if (spawningShell) return;
    spawningShell = true;
    spawnError = null;
    try {
      let workdir = '.';
      try {
        const home = await api.listDir();
        if (home?.path) workdir = home.path;
      } catch {
        // Fall through to "."; the backend will reject if it's invalid.
      }
      const created = await api.createSession({
        name: nextShellName(),
        workdir,
        tool: 'bash',
        model: null,
        flags: []
      });
      try {
        await api.startSession(created.id);
      } catch (e) {
        spawnError = e instanceof Error ? e.message : String(e);
        await loadSessions();
        return;
      }
      await loadSessions();
      await goto(`/sessions/${created.id}`);
    } catch (e) {
      spawnError = e instanceof Error ? e.message : String(e);
    } finally {
      spawningShell = false;
    }
  }
</script>

<section class="head">
  <div class="head-text">
    <span class="eyebrow">Overview</span>
    <h1 class="display-1">Sessions</h1>
    <p class="lede">All registered AI agent sessions on this host.</p>
  </div>
  <div class="head-actions">
    <button
      type="button"
      class="filter mono"
      class:on={runningOnly}
      onclick={toggleRunning}
      title="Show only running sessions (CLI: agentum ps)"
    >
      <span class="status-dot" data-status={runningOnly ? 'running' : 'idle'} aria-hidden="true"></span>
      {runningOnly ? 'running only' : 'all sessions'}
    </button>
    <button
      type="button"
      class="btn btn-shell"
      onclick={spawnShell}
      disabled={spawningShell}
      title="Spawn a plain bash shell session (mirrors TUI `t`)"
    >
      <span class="shell-glyph" aria-hidden="true">›_</span>
      {spawningShell ? 'spawning…' : 'Spawn shell'}
    </button>
    <button class="btn btn-cta" onclick={openNewSession}>
      <span class="plus">+</span> New session
    </button>
  </div>
</section>

{#if spawnError}
  <div class="banner mono" role="alert">
    <span class="banner-tag">SHELL</span>
    <span>Failed to spawn shell: <code>{spawnError}</code></span>
  </div>
{/if}

{#if $sessions.items.length > 0}
  <div class="stats">
    <div class="stat-tile" data-tone="default">
      <span class="stat-label mono">Total</span>
      <span class="stat-num">{stats.total}</span>
    </div>
    <div class="stat-tile" data-tone="success" class:active={stats.running > 0}>
      <span class="stat-label mono">Running</span>
      <span class="stat-num">{stats.running}</span>
    </div>
    <div class="stat-tile" data-tone="muted">
      <span class="stat-label mono">Stopped</span>
      <span class="stat-num">{stats.stopped}</span>
    </div>
    <div class="stat-tile" data-tone="danger" class:active={stats.crashed > 0}>
      <span class="stat-label mono">Crashed</span>
      <span class="stat-num">{stats.crashed}</span>
    </div>
    <div class="stat-tile" data-tone="default">
      <span class="stat-label mono">Tools</span>
      <span class="stat-num">{stats.tools}</span>
    </div>
    {#if health}
      <div class="stat-tile" data-tone="default">
        <span class="stat-label mono">Uptime</span>
        <span class="stat-num">{fmtUptime(health.uptime_seconds)}</span>
      </div>
    {/if}
  </div>
{/if}

{#if $sessions.error}
  <div class="banner mono" role="alert">
    <span class="banner-tag">ERROR</span>
    <span>Failed to load sessions: <code>{$sessions.error}</code></span>
  </div>
{:else if $sessions.loading && $sessions.items.length === 0}
  <div class="grid"><Skeleton rows={6} height="5rem" /></div>
{:else if $sessions.items.length === 0}
  {#if runningOnly}
    <EmptyState
      eyebrow="No matches"
      title="No running sessions"
      body="Start one from the all-sessions view, or create a new one."
      cta={{ label: '+ New session', onclick: openNewSession }}
    />
  {:else}
    <EmptyState
      eyebrow="Empty"
      title="No sessions yet"
      body="Create your first agent session — from this dashboard, or your terminal."
      cmd="agentum new alpha --tool claude --dir ~/projects/foo --up"
      cta={{ label: '+ New session', onclick: openNewSession }}
    />
  {/if}
{:else}
  <div class="grid">
    {#each $sessions.items as session (session.id)}
      <SessionCard {session} onChanged={refresh} />
    {/each}
  </div>
{/if}

{#if recentActivity.length > 0}
  <section class="activity">
    <div class="activity-head">
      <span class="eyebrow">Activity</span>
      <h2 class="display-2">Recent</h2>
    </div>
    <div class="activity-list">
      {#each recentActivity as s (s.id)}
        <a class="activity-row" href={`/sessions/${s.id}`}>
          <span class="status-dot" data-status={s.status} aria-hidden="true"></span>
          <span class="act-name">{s.name}</span>
          <span class="act-tool mono">{s.tool}</span>
          <span class="act-status mono" data-status={s.status}>{s.status}</span>
          <span class="act-time mono">{fmtRel(s.last_activity_at)}</span>
        </a>
      {/each}
    </div>
  </section>
{/if}

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    margin: 8px 0 32px;
    gap: 24px;
    flex-wrap: wrap;
  }
  .head-text { display: flex; flex-direction: column; gap: 8px; }
  .head-text .eyebrow { margin-bottom: 4px; }
  .lede {
    margin: 0;
    color: var(--text-2);
    font-size: 15px;
    line-height: 1.5;
    max-width: 60ch;
  }

  .head-actions { display: flex; gap: 10px; align-items: center; }

  .filter {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 14px;
    background: var(--surface);
    color: var(--text-2);
    border: 1px solid var(--border-2);
    border-radius: 99999px;
    font-size: 12px;
    cursor: pointer;
    letter-spacing: 0.02em;
    transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
  }
  .filter:hover { color: var(--text); border-color: var(--accent); }
  .filter.on {
    color: var(--success);
    border-color: color-mix(in srgb, var(--success) 35%, var(--border-2));
  }

  .plus {
    display: inline-block;
    font-weight: 500;
    margin-right: 2px;
  }

  /* Plain-shell quick-spawn button — secondary CTA next to "+ New session". */
  .btn-shell {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 14px;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border-2);
    border-radius: 99999px;
    font-family: var(--font-mono, inherit);
    font-size: 12px;
    letter-spacing: 0.02em;
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
  }
  .btn-shell:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--accent);
  }
  .btn-shell:disabled { opacity: 0.55; cursor: not-allowed; }
  .shell-glyph {
    font-family: var(--font-mono, monospace);
    color: var(--accent);
    font-weight: 600;
    letter-spacing: -0.05em;
  }

  /* ---------- stat tiles ---------- */
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 8px;
    margin-bottom: 28px;
  }
  .stat-tile {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px 16px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius);
    transition: border-color 120ms ease;
  }
  .stat-tile:hover { border-color: var(--accent); }
  .stat-label {
    font-size: 10.5px;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
  }
  .stat-num {
    font-family: var(--font-display);
    font-size: 28px;
    font-weight: 600;
    color: var(--text);
    letter-spacing: -0.02em;
    line-height: 1;
  }
  .stat-tile[data-tone="success"].active .stat-num { color: var(--success); }
  .stat-tile[data-tone="danger"].active .stat-num  { color: var(--danger); }
  .stat-tile[data-tone="muted"] .stat-num          { color: var(--text-2); }

  /* ---------- grid ---------- */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 12px;
  }

  /* ---------- error banner ---------- */
  .banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: color-mix(in srgb, var(--danger) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--danger) 35%, var(--border-2));
    border-radius: var(--radius);
    color: var(--text);
    font-size: 13px;
    margin-bottom: 16px;
  }
  .banner-tag {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    padding: 3px 7px;
    border-radius: 99999px;
    background: var(--danger);
    color: #fff;
    font-weight: 600;
  }
  .banner code { color: var(--text-2); }

  /* ---------- activity ---------- */
  .activity { margin-top: 48px; padding-top: 32px; border-top: 1px solid var(--border-2); }
  .activity-head { display: flex; flex-direction: column; gap: 6px; margin-bottom: 16px; }

  .activity-list { display: flex; flex-direction: column; }
  .activity-row {
    display: grid;
    grid-template-columns: 12px 1fr auto auto auto;
    align-items: center;
    gap: 16px;
    padding: 12px 14px;
    color: var(--text);
    text-decoration: none;
    font-size: 13px;
    border-bottom: 1px solid var(--border);
    transition: background 120ms ease, color 120ms ease;
  }
  .activity-row:last-child { border-bottom: 0; }
  .activity-row:hover { background: var(--surface); color: var(--text); }
  .act-name { font-weight: 500; letter-spacing: -0.005em; }
  .act-tool { color: var(--cta); font-size: 11.5px; letter-spacing: 0.02em; }
  .act-status {
    font-size: 10.5px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .act-status[data-status="running"] { color: var(--success); }
  .act-status[data-status="crashed"] { color: var(--danger); }
  .act-time { font-size: 11px; color: var(--muted); }

  @media (max-width: 720px) {
    .head { gap: 14px; }
    .head-actions { width: 100%; }
    .head-actions .btn { flex: 1; }
    .activity-row {
      grid-template-columns: 12px 1fr auto;
      grid-template-rows: auto auto;
    }
    .act-tool, .act-status { display: none; }
  }
</style>
