<script lang="ts">
  /**
   * SessionGitPanel — ORCA §2/§3 diff viewer + staging + commit UI.
   *
   * Renders `git status` for a session's worktree in three groups
   * (Staged · Changes · Untracked), lets the user click a path to preview
   * its diff in a CodeMirror side-by-side view (see DiffView.svelte), move
   * files in/out of the index with Stage/Unstage, and commit the staged set
   * with a message. Backed by the routes in
   * `crates/agentum-server/src/routes/git.rs`:
   *
   *   GET  /api/sessions/{id}/git/status
   *   GET  /api/sessions/{id}/git/file?path=&rev=head|index|worktree
   *   POST /api/sessions/{id}/git/stage      { paths, unstage }
   *   POST /api/sessions/{id}/git/commit     { message, paths }
   *
   * Model: the three groups mirror git's real index. "Stage"/"Unstage" call
   * `/git/stage` to move a path between index and worktree; "Commit" ships
   * exactly what's staged (`status.staged`), so what you see is what commits.
   *
   * Polls `/git/status` every 5 s while expanded; pauses when collapsed.
   */
  import { onDestroy } from 'svelte';
  import { api, ApiError, type GitStatus, type Session } from '$lib/api';
  import DiffView from './DiffView.svelte';

  interface Props { session: Session; }
  let { session }: Props = $props();

  let status = $state<GitStatus | null>(null);
  let error = $state<string | null>(null);

  /** Currently previewed file. `staged` decides which two revisions to
   *  diff: staged → HEAD ↔ index; otherwise index ↔ working tree. */
  let viewing = $state<{ path: string; staged: boolean } | null>(null);
  let orig = $state('');
  let modified = $state('');
  let truncated = $state(false);
  let diffLoading = $state(false);

  let message = $state('');
  let committing = $state(false);
  let busy = $state(false);

  let pollId: ReturnType<typeof setInterval> | null = null;

  async function refresh() {
    try {
      status = await api.gitStatus(session.id);
      error = null;
    } catch (e) {
      if (e instanceof ApiError && e.status === 400) {
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
    if (open) { refresh(); startPolling(); }
    else { stopPolling(); }
  }

  async function view(path: string, staged: boolean) {
    viewing = { path, staged };
    diffLoading = true;
    orig = '';
    modified = '';
    truncated = false;
    try {
      // staged view diffs HEAD↔index; unstaged/untracked diffs index↔worktree.
      // (Untracked files have no index/HEAD blob → that side returns empty,
      // rendering as an all-added file.)
      const [a, b] = staged
        ? await Promise.all([
            api.gitFile(session.id, path, 'head'),
            api.gitFile(session.id, path, 'index')
          ])
        : await Promise.all([
            api.gitFile(session.id, path, 'index'),
            api.gitFile(session.id, path, 'worktree')
          ]);
      orig = a.content;
      modified = b.content;
      truncated = a.truncated || b.truncated;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      viewing = null;
    } finally {
      diffLoading = false;
    }
  }

  function closeDiff() {
    viewing = null;
    orig = '';
    modified = '';
  }

  async function setStaged(path: string, unstage: boolean) {
    if (busy) return;
    busy = true;
    try {
      status = await api.gitStage(session.id, [path], unstage);
      error = null;
      // If the previewed file moved sides, re-fetch the diff for the new side.
      if (viewing?.path === path) await view(path, !unstage);
    } catch (e) {
      error = e instanceof Error ? e.message.replace(/^HTTP \d+:\s*/, '') : String(e);
    } finally {
      busy = false;
    }
  }

  async function commit() {
    const paths = status?.staged ?? [];
    if (!message.trim() || paths.length === 0) return;
    committing = true;
    error = null;
    try {
      await api.gitCommit(session.id, message.trim(), paths);
      message = '';
      closeDiff();
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      committing = false;
    }
  }

  const stagedCount = $derived(status?.staged.length ?? 0);
  const canCommit = $derived(!committing && stagedCount > 0 && message.trim().length > 0);
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
                <button type="button" class="act" disabled={busy}
                        title="Unstage (git restore --staged)"
                        onclick={() => setStaged(path, true)}>Unstage</button>
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
                <button type="button" class="act" disabled={busy}
                        title="Stage (git add)"
                        onclick={() => setStaged(path, false)}>Stage</button>
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
                <button type="button" class="act" disabled={busy}
                        title="Stage (git add)"
                        onclick={() => setStaged(path, false)}>Stage</button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      {#if viewing}
        <section class="diff">
          <header>
            <span class="mono">{viewing.path}</span>
            <span class="side">{viewing.staged ? '(HEAD ↔ index)' : '(index ↔ working tree)'}</span>
            <span class="spacer"></span>
            {#if truncated}<span class="side">truncated</span>{/if}
            <button type="button" class="act" onclick={closeDiff}>close</button>
          </header>
          {#if diffLoading}
            <div class="loading empty">loading…</div>
          {:else}
            <DiffView original={orig} modified={modified} filename={viewing.path} />
          {/if}
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
          {committing ? 'committing…' : `Commit (${stagedCount})`}
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
  .empty {
    font-size: 11px;
    color: var(--fg-3);
    padding-left: 14px;
  }
  .loading { padding: 8px 10px; }
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
  .act:disabled { opacity: 0.5; cursor: not-allowed; }

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
