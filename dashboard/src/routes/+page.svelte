<script lang="ts">
  import { onMount } from 'svelte';
  import { sessions, loadSessions } from '$stores/sessions';
  import { openNewSession } from '$stores/newSession';
  import SessionCard from '$components/SessionCard.svelte';
  import EmptyState from '$components/EmptyState.svelte';
  import Skeleton from '$components/Skeleton.svelte';
  import Icon from '$components/Icon.svelte';
  import { api, type Health } from '$lib/api';

  let runningOnly = $state(false);
  let health = $state<Health | null>(null);

  function refresh() {
    loadSessions(runningOnly ? 'running' : undefined);
  }

  async function loadHealth() {
    try {
      health = await api.health();
    } catch { /* non-critical */ }
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
</script>

<section class="head">
  <div>
    <h2>Sessions</h2>
    <p class="muted">All registered AI agent sessions on this host.</p>
  </div>
  <div class="actions">
    <button
      type="button"
      class="filter"
      class:on={runningOnly}
      onclick={toggleRunning}
      title="Show only running sessions (CLI: agentum ps)"
    >
      {runningOnly ? 'running only ●' : 'all sessions'}
    </button>
    <button class="primary" onclick={openNewSession}>+ New</button>
  </div>
</section>

{#if $sessions.items.length > 0}
  <div class="stat-cards">
    <div class="stat-card">
      <Icon name="monitor" size={16} />
      <div class="stat-value">{stats.total}</div>
      <div class="stat-label">total</div>
    </div>
    <div class="stat-card accent">
      <Icon name="activity" size={16} />
      <div class="stat-value">{stats.running}</div>
      <div class="stat-label">running</div>
    </div>
    <div class="stat-card warn">
      <Icon name="terminal" size={16} />
      <div class="stat-value">{stats.stopped}</div>
      <div class="stat-label">stopped</div>
    </div>
    <div class="stat-card danger">
      <Icon name="zap" size={16} />
      <div class="stat-value">{stats.crashed}</div>
      <div class="stat-label">crashed</div>
    </div>
    <div class="stat-card">
      <Icon name="tool" size={16} />
      <div class="stat-value">{stats.tools}</div>
      <div class="stat-label">tools</div>
    </div>
    {#if health}
      <div class="stat-card">
        <Icon name="cpu" size={16} />
        <div class="stat-value">{Math.floor(health.uptime_seconds / 3600)}h</div>
        <div class="stat-label">uptime</div>
      </div>
    {/if}
  </div>
{/if}

{#if $sessions.error}
  <div class="error">Failed to load sessions: <code>{$sessions.error}</code></div>
{:else if $sessions.loading && $sessions.items.length === 0}
  <div class="grid"><Skeleton rows={6} height="5rem" /></div>
{:else if $sessions.items.length === 0}
  {#if runningOnly}
    <EmptyState
      title="No running sessions"
      body="Start one from the all-sessions view, or create a new one."
    />
  {:else}
    <EmptyState
      title="No sessions yet"
      body="Click + New above, or run from your terminal:"
      cmd="agentum new alpha --tool claude --dir ~/projects/foo --up"
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
  <section class="activity-section">
    <h3>Recent Activity</h3>
    <div class="activity-list">
      {#each recentActivity as s (s.id)}
        <a class="activity-row" href={`/sessions/${s.id}`}>
          <span class="act-dot" class:running={s.status === 'running'} class:crashed={s.status === 'crashed'}></span>
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
    margin-bottom: 1rem;
    gap: 1rem;
    flex-wrap: wrap;
  }
  h2 {
    font-family: var(--font-display);
    font-weight: 600;
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  h3 {
    font-family: var(--font-display);
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--text-2);
    margin: 0 0 0.5rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .muted { color: var(--muted); margin: 0; }
  .mono { font-family: var(--font-mono); }
  .actions { display: flex; gap: 0.5rem; align-items: center; }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 0.9rem;
  }
  .primary {
    background: var(--accent);
    color: var(--bg);
    border: 1px solid var(--accent);
    padding: 0.5rem 1rem;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
    cursor: pointer;
    transition: filter var(--transition, 150ms ease);
  }
  .primary:hover { filter: brightness(1.15); }
  .filter {
    background: var(--surface);
    color: var(--text-2);
    border: 1px solid var(--border);
    padding: 0.45rem 0.85rem;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.78rem;
    cursor: pointer;
    transition: color var(--transition, 150ms ease), border-color var(--transition, 150ms ease);
  }
  .filter:hover { color: var(--text); }
  .filter.on {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
    background: color-mix(in srgb, var(--accent) 8%, var(--surface));
  }
  .error {
    padding: 0.8rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    margin-bottom: 1rem;
  }
  .error code { font-family: var(--font-mono); color: var(--text); }

  /* Stat cards */
  .stat-cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(100px, 1fr));
    gap: 0.5rem;
    margin-bottom: 1.25rem;
  }
  .stat-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.15rem;
    padding: 0.6rem 0.5rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text-2);
    transition: border-color var(--transition, 150ms ease);
  }
  .stat-card:hover { border-color: color-mix(in srgb, var(--accent) 20%, var(--border)); }
  .stat-card.accent { color: var(--accent); }
  .stat-card.warn { color: var(--warn); }
  .stat-card.danger { color: var(--danger); }
  .stat-value {
    font-family: var(--font-display);
    font-size: 1.3rem;
    font-weight: 700;
    color: var(--text);
  }
  .stat-label {
    font-family: var(--font-mono);
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .stat-card.accent .stat-value { color: var(--accent); }
  .stat-card.warn .stat-value { color: var(--warn); }
  .stat-card.danger .stat-value { color: var(--danger); }

  /* Activity */
  .activity-section {
    margin-top: 1.5rem;
    padding-top: 1rem;
    border-top: 1px solid var(--border);
  }
  .activity-list {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .activity-row {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.5rem 0.7rem;
    border-radius: 6px;
    color: var(--text);
    text-decoration: none;
    transition: background var(--transition, 150ms ease);
    font-size: 0.85rem;
  }
  .activity-row:hover { background: var(--surface); }
  .act-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--muted);
    flex-shrink: 0;
  }
  .act-dot.running { background: var(--success); box-shadow: 0 0 4px var(--success); }
  .act-dot.crashed { background: var(--danger); }
  .act-name { flex: 1; font-weight: 500; }
  .act-tool { color: var(--accent); font-size: 0.78rem; }
  .act-status { font-size: 0.72rem; color: var(--muted); }
  .act-status[data-status="running"] { color: var(--success); }
  .act-status[data-status="crashed"] { color: var(--danger); }
  .act-time { font-size: 0.72rem; color: var(--muted); }
</style>
