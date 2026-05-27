<script lang="ts">
  /**
   * SessionGitPanel — ORCA §3 (P1) diff viewer + commit UI.
   *
   * Renders `git status` for a session's worktree in three groups
   * (Staged · Changes · Untracked), lets the user click a path to
   * preview its unified diff inline, toggle inclusion in the next
   * commit with Stage/Unstage buttons, and ship a commit by typing a
   * message + clicking Commit. Backed by the three routes added in
   * `crates/agentum-server/src/routes/git.rs`:
   *
   *   GET  /api/sessions/{id}/git/status
   *   GET  /api/sessions/{id}/git/diff?path=&staged=
   *   POST /api/sessions/{id}/git/commit
   *
   * State model: server-side "Staged" files are pre-included in the
   * commit set; unstaged + untracked default to excluded. The commit
   * POST sends only the paths the user explicitly opted in, and the
   * server uses `git commit -- <paths>` so siblings staged outside
   * agentum don't get pulled in by surprise.
   *
   * Polls `/git/status` every 5 s while expanded; pauses when the
   * <details> is closed to keep the panel zero-cost when idle.
   */
  import { onDestroy } from 'svelte';
  import { api, ApiError, type GitStatus, type Session } from '$lib/api';

  interface Props { session: Session; }
  let { session }: Props = $props();

  let status = $state<GitStatus | null>(null);
  let error = $state<string | null>(null);

  /** Set of paths the user wants in the next commit. Staged paths
   *  auto-populate on each refresh (see syncSelection). */
  let selected = $state<Set<string>>(new Set());

  /** Currently previewed file. `staged` decides which side of the
   *  diff to ask for (worktree vs index, or index vs HEAD). */
  let viewing = $state<{ path: string; staged: boolean } | null>(null);
  let diffText = $state<string>('');
  let diffLoading = $state(false);

  let message = $state('');
  let committing = $state(false);

  let pollId: ReturnType<typeof setInterval> | null = null;

  /** Auto-include freshly-discovered staged files in the next commit
   *  set, but leave the user's manual toggles alone. Removes paths
   *  from `selected` that no longer exist anywhere in the status so
   *  the count stays accurate after a commit. */
  function syncSelection(s: GitStatus) {
    const next = new Set(selected);
    for (const p of s.staged) next.add(p);
    const live = new Set([...s.staged, ...s.unstaged, ...s.untracked]);
    for (const p of next) {
      if (!live.has(p)) next.delete(p);
    }
    selected = next;
  }

  async function refresh() {
    try {
      const s = await api.gitStatus(session.id);
      status = s;
      error = null;
      syncSelection(s);
    } catch (e) {
      if (e instanceof ApiError && e.status === 400) {
        // "not a git repository" — surface plainly and bail.
        error = e.message.replace(/^HTTP \d+:\s*/, '');
        status = null;
      } else {
        error = e instanceof Error ? e.message : String(e);
      }
    }
  }

  function startPolling() {
    if (pollId) return;
    pollId = setInterval(refresh, 5000);
  }
  function stopPolling() {
    if (pollId) { clearInterval(pollId); pollId = null; }
  }

  function onToggle(e: Event) {
    const open = (e.currentTarget as HTMLDetailsElement).open;
    if (open) {
      refresh();
      startPolling();
    } else {
      stopPolling();
    }
  }

  async function view(path: string, staged: boolean) {
    viewing = { path, staged };
    diffLoading = true;
    diffText = '';
    try {
      diffText = await api.gitDiff(session.id, path, staged);
    } catch (e) {
      diffText = `error: ${e instanceof Error ? e.message : String(e)}`;
    } finally {
      diffLoading = false;
    }
  }

  function toggleSelection(path: string) {
    const next = new Set(selected);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    selected = next;
  }

  async function commit() {
    if (!message.trim()) return;
    const paths = Array.from(selected);
    if (paths.length === 0) return;
    committing = true;
    error = null;
    try {
      await api.gitCommit(session.id, message.trim(), paths);
      message = '';
      selected = new Set();
      viewing = null;
      diffText = '';
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      committing = false;
    }
  }

  /** Classify a unified-diff line for inline coloring. Keeps the
   *  renderer in this file so the panel ships as a single component —
   *  the prompt left syntax highlighting optional and prismjs would
   *  pull in a non-trivial bundle for this lightweight surface. */
  type DiffLineKind = 'add' | 'del' | 'hunk' | 'meta' | 'ctx';
  function classify(line: string): DiffLineKind {
    if (line.startsWith('@@')) return 'hunk';
    if (line.startsWith('diff ') ||
        line.startsWith('index ') ||
        line.startsWith('--- ') ||
        line.startsWith('+++ ') ||
        line.startsWith('new file') ||
        line.startsWith('deleted file') ||
        line.startsWith('similarity ') ||
        line.startsWith('rename ')) return 'meta';
    if (line.startsWith('+')) return 'add';
    if (line.startsWith('-')) return 'del';
    return 'ctx';
  }

  const diffLines = $derived(diffText ? diffText.split('\n') : []);
  const totalSelected = $derived(selected.size);
  const canCommit = $derived(
    !committing && totalSelected > 0 && message.trim().length > 0
  );
  const summary = $derived.by(() => {
    if (!status) return 'Git';
    const s = status.staged.length;
    const u = status.unstaged.length;
    const n = status.untracked.length;
    if (s + u + n === 0) return 'Git · clean';
    return `Git · ${s} staged · ${u} changed · ${n} new`;
  });

  onDestroy(() => { stopPolling(); });
</script>

<details class="git-section" ontoggle={onToggle}>
  <summary>
    <span class="caret">▸</span>
    <span class="label">{summary}</span>
  </summary>

  <div class="body">
    {#if error}
      <div class="err mono" role="alert">{error}</div>
    {/if}

    {#if status}
      <section class="group">
        <header><span class="dot staged"></span>Staged <span class="ct">{status.staged.length}</span></header>
        {#if status.staged.length === 0}
          <div class="empty">—</div>
        {:else}
          <ul>
            {#each status.staged as path (`s:${path}`)}
              <li class:active={viewing?.path === path && viewing?.staged}>
                <button type="button" class="row" onclick={() => view(path, true)}>
                  <span class="sigil add">+</span>
                  <span class="path mono">{path}</span>
                </button>
                <button type="button"
                        class="act"
                        title={selected.has(path) ? 'Exclude from next commit' : 'Include in next commit'}
                        onclick={() => toggleSelection(path)}>
                  {selected.has(path) ? 'Unstage' : 'Stage'}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <section class="group">
        <header><span class="dot unstaged"></span>Changes <span class="ct">{status.unstaged.length}</span></header>
        {#if status.unstaged.length === 0}
          <div class="empty">—</div>
        {:else}
          <ul>
            {#each status.unstaged as path (`u:${path}`)}
              <li class:active={viewing?.path === path && !viewing?.staged}>
                <button type="button" class="row" onclick={() => view(path, false)}>
                  <span class="sigil mod">~</span>
                  <span class="path mono">{path}</span>
                </button>
                <button type="button"
                        class="act"
                        onclick={() => toggleSelection(path)}>
                  {selected.has(path) ? 'Unstage' : 'Stage'}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <section class="group">
        <header><span class="dot untracked"></span>Untracked <span class="ct">{status.untracked.length}</span></header>
        {#if status.untracked.length === 0}
          <div class="empty">—</div>
        {:else}
          <ul>
            {#each status.untracked as path (`n:${path}`)}
              <li class:active={viewing?.path === path && !viewing?.staged}>
                <button type="button" class="row" onclick={() => view(path, false)}>
                  <span class="sigil new">?</span>
                  <span class="path mono">{path}</span>
                </button>
                <button type="button"
                        class="act"
                        onclick={() => toggleSelection(path)}>
                  {selected.has(path) ? 'Unstage' : 'Stage'}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      {#if viewing}
        <section class="diff">
          <header>
            <span class="mono">{viewing.path}</span>
            <span class="side">{viewing.staged ? '(staged)' : '(working tree)'}</span>
            <span class="spacer"></span>
            <button type="button" class="act" onclick={() => { viewing = null; diffText = ''; }}>close</button>
          </header>
          <div class="diff-body mono">
            {#if diffLoading}
              <div class="empty">loading…</div>
            {:else if !diffText}
              <div class="empty">no diff</div>
            {:else}
              {#each diffLines as line, i (i)}
                <div class="dl {classify(line)}">{line || ' '}</div>
              {/each}
            {/if}
          </div>
        </section>
      {/if}

      <section class="commit">
        <input type="text"
               bind:value={message}
               placeholder="commit message…"
               disabled={committing} />
        <button type="button" class="tb-btn primary"
                disabled={!canCommit}
                onclick={commit}>
          {committing ? 'committing…' : `Commit (${totalSelected})`}
        </button>
      </section>
    {/if}
  </div>
</details>

<style>
  .git-section {
    border-top: 1px solid var(--border);
    background: var(--bg-2);
  }
  .git-section > summary {
    cursor: pointer;
    list-style: none;
    padding: 8px 16px;
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--fg-2);
    font-size: 12px;
    user-select: none;
  }
  .git-section > summary::-webkit-details-marker { display: none; }
  .git-section > summary:hover { color: var(--fg); }
  .git-section[open] > summary .caret { transform: rotate(90deg); }
  .caret {
    display: inline-block;
    transition: transform 120ms ease;
    color: var(--fg-3);
    font-size: 10px;
  }
  .label { color: var(--fg); }

  .body {
    padding: 8px 16px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .err {
    padding: 8px 10px;
    border: 1px solid rgba(255,85,85,0.45);
    background: rgba(255,85,85,0.07);
    color: var(--crash);
    border-radius: var(--radius);
    font-size: 11px;
  }

  .group header {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
    margin-bottom: 4px;
  }
  .group .ct {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
    color: var(--fg-2);
  }
  .group .empty {
    font-size: 11px;
    color: var(--fg-3);
    padding-left: 14px;
  }
  .group ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .group li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 0;
  }
  .group li.active .row { background: rgba(99,149,255,0.08); }

  .row {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 6px;
    background: transparent;
    border: 0;
    padding: 3px 6px;
    border-radius: 3px;
    color: var(--fg);
    cursor: pointer;
    text-align: left;
    min-width: 0;
  }
  .row:hover { background: var(--bg-tb-hover); }
  .path {
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sigil {
    width: 10px;
    text-align: center;
    font-family: var(--mono);
    font-size: 10.5px;
    font-weight: 600;
  }
  .sigil.add { color: var(--green); }
  .sigil.mod { color: var(--amber); }
  .sigil.new { color: var(--link); }

  .dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }
  .dot.staged    { background: var(--green); }
  .dot.unstaged  { background: var(--amber); }
  .dot.untracked { background: var(--link); }

  .act {
    background: transparent;
    border: 1px solid var(--border-2);
    color: var(--fg-2);
    padding: 1px 7px;
    border-radius: 3px;
    font-size: 10.5px;
    cursor: pointer;
    font-family: var(--mono);
  }
  .act:hover { color: var(--fg); border-color: var(--fg-3); }

  .diff {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: #050505;
  }
  .diff header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-chrome);
    border-bottom: 1px solid var(--border);
    font-size: 11px;
  }
  .diff header .side { color: var(--fg-3); font-size: 10.5px; }
  .diff header .spacer { flex: 1; }
  .diff-body {
    max-height: 360px;
    overflow: auto;
    padding: 6px 0;
    font-size: 11.5px;
    line-height: 1.45;
  }
  .dl {
    padding: 0 10px;
    white-space: pre;
  }
  .dl.add  { background: rgba(25,214,0,0.08); color: var(--green); }
  .dl.del  { background: rgba(221,0,0,0.10);  color: var(--crash); }
  .dl.hunk { background: rgba(99,149,255,0.08); color: var(--link); }
  .dl.meta { color: var(--fg-3); }
  .dl.ctx  { color: var(--fg-2); }

  .commit {
    display: flex;
    gap: 8px;
    align-items: center;
    padding-top: 4px;
  }
  .commit input {
    flex: 1;
    height: 28px;
    padding: 0 10px;
    background: var(--bg);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .commit input:disabled { opacity: 0.5; cursor: not-allowed; }

  /* Phone: stack stage/unstage buttons under the path so they don't
     crowd a narrow viewport. */
  @media (max-width: 540px) {
    .group li { flex-wrap: wrap; }
    .act { margin-left: 16px; }
  }
</style>
