<script lang="ts">
  /**
   * ForgePanel — ORCA §2 GitHub/GitLab integration (P1).
   *
   * For a session whose `origin` remote points at GitHub or GitLab, shows
   * the open PRs/MRs, open issues, and CI checks for the current branch,
   * plus a "New PR" action. Backed by the routes in
   * `crates/agentum-server/src/routes/forge.rs`:
   *
   *   GET  /api/sessions/{id}/forge/info
   *   GET  /api/sessions/{id}/forge/prs | /issues | /checks?ref=
   *   POST /api/sessions/{id}/forge/pr
   *   GET/PUT /api/forge/token
   *
   * The PAT lives only on the daemon (`<data_dir>/forge.json`, 0600); the
   * client only ever learns whether one is set. When no token is stored
   * the panel shows an inline "connect" form instead of the lists.
   *
   * Like SessionGitPanel, it loads on <details> expand and refreshes every
   * 30 s while open (forge APIs are rate-limited, so a slower cadence than
   * the local git poll).
   */
  import { onDestroy } from 'svelte';
  import {
    api,
    ApiError,
    type Session,
    type ForgeInfo,
    type ForgePr,
    type ForgeIssue,
    type ForgeCheck
  } from '$lib/api';

  interface Props { session: Session; }
  let { session }: Props = $props();

  let info = $state<ForgeInfo | null>(null);
  let prs = $state<ForgePr[]>([]);
  let issues = $state<ForgeIssue[]>([]);
  let checks = $state<ForgeCheck[]>([]);
  let error = $state<string | null>(null);
  let loading = $state(false);

  // Token entry (shown when the forge is detected but no PAT is stored).
  let tokenInput = $state('');
  let savingToken = $state(false);

  // New-PR mini form.
  let prTitle = $state('');
  let prBase = $state('main');
  let creating = $state(false);
  let createdUrl = $state<string | null>(null);

  let pollId: ReturnType<typeof setInterval> | null = null;

  async function loadLists() {
    if (!info?.forge || !info.has_token) return;
    loading = true;
    try {
      const [p, i, c] = await Promise.all([
        api.forgePrs(session.id),
        api.forgeIssues(session.id),
        api.forgeChecks(session.id)
      ]);
      prs = p;
      issues = i;
      checks = c;
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message.replace(/^HTTP \d+:\s*/, '') : String(e);
    } finally {
      loading = false;
    }
  }

  async function refresh() {
    try {
      info = await api.forgeInfo(session.id);
      error = null;
      await loadLists();
    } catch (e) {
      if (e instanceof ApiError && e.status === 400) {
        error = e.message.replace(/^HTTP \d+:\s*/, '');
        info = null;
      } else {
        error = e instanceof Error ? e.message : String(e);
      }
    }
  }

  function startPolling() {
    if (pollId) return;
    pollId = setInterval(refresh, 30000);
  }
  function stopPolling() {
    if (pollId) { clearInterval(pollId); pollId = null; }
  }

  function onToggle(e: Event) {
    const open = (e.currentTarget as HTMLDetailsElement).open;
    if (open) { refresh(); startPolling(); }
    else { stopPolling(); }
  }

  async function saveToken() {
    if (!info?.forge || !tokenInput.trim()) return;
    savingToken = true;
    try {
      await api.forgeSetToken(info.forge, tokenInput.trim());
      tokenInput = '';
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      savingToken = false;
    }
  }

  async function createPr() {
    if (!prTitle.trim() || !prBase.trim()) return;
    creating = true;
    createdUrl = null;
    try {
      const { url } = await api.forgeCreatePr(session.id, prTitle.trim(), prBase.trim());
      createdUrl = url;
      prTitle = '';
      await loadLists();
    } catch (e) {
      error = e instanceof Error ? e.message.replace(/^HTTP \d+:\s*/, '') : String(e);
    } finally {
      creating = false;
    }
  }

  const forgeLabel = $derived(info?.forge === 'github' ? 'GitHub' : info?.forge === 'gitlab' ? 'GitLab' : null);
  const summary = $derived.by(() => {
    if (!info?.forge) return 'Forge';
    return info.has_token
      ? `${forgeLabel} · ${prs.length} PR · ${issues.length} issues`
      : `${forgeLabel} · connect`;
  });

  function checkClass(status: string): string {
    if (status === 'success') return 'ok';
    if (status === 'failure') return 'fail';
    return 'pending';
  }

  onDestroy(() => { stopPolling(); });
</script>

<details class="forge-section" ontoggle={onToggle}>
  <summary>
    <span class="caret">▸</span>
    <span class="label">{summary}</span>
  </summary>

  <div class="body">
    {#if error}
      <div class="err mono" role="alert">{error}</div>
    {/if}

    {#if info && !info.forge && !error}
      <div class="empty">No GitHub/GitLab remote on this session.</div>
    {/if}

    {#if info?.forge}
      <div class="repo mono">
        {forgeLabel}: {info.project ?? '—'}{#if info.branch} · <span class="branch">{info.branch}</span>{/if}
      </div>

      {#if !info.has_token}
        <!-- No PAT stored → inline connect form. Sent over the loopback/TLS
             API and persisted 0600 on the daemon; never echoed back. -->
        <div class="connect">
          <p class="hint">
            Add a {forgeLabel} personal access token to list PRs, issues and checks.
          </p>
          <div class="row-form">
            <input
              type="password"
              placeholder={info.forge === 'github' ? 'ghp_… or github_pat_…' : 'glpat-…'}
              bind:value={tokenInput}
              disabled={savingToken} />
            <button type="button" class="primary" onclick={saveToken} disabled={savingToken || !tokenInput.trim()}>
              {savingToken ? 'Saving…' : 'Save token'}
            </button>
          </div>
        </div>
      {:else}
        <section class="group">
          <header>Pull requests <span class="ct">{prs.length}</span></header>
          {#if prs.length === 0}
            <div class="empty">{loading ? 'Loading…' : '—'}</div>
          {:else}
            <ul>
              {#each prs as pr (`pr:${pr.number}`)}
                <li>
                  <a class="row" href={pr.url} target="_blank" rel="noreferrer">
                    <span class="num mono">#{pr.number}</span>
                    <span class="title">{pr.title}</span>
                    {#if pr.draft}<span class="tag">draft</span>{/if}
                  </a>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section class="group">
          <header>Issues <span class="ct">{issues.length}</span></header>
          {#if issues.length === 0}
            <div class="empty">{loading ? 'Loading…' : '—'}</div>
          {:else}
            <ul>
              {#each issues as it (`is:${it.number}`)}
                <li>
                  <a class="row" href={it.url} target="_blank" rel="noreferrer">
                    <span class="num mono">#{it.number}</span>
                    <span class="title">{it.title}</span>
                  </a>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section class="group">
          <header>Checks <span class="ct">{checks.length}</span></header>
          {#if checks.length === 0}
            <div class="empty">{loading ? 'Loading…' : '—'}</div>
          {:else}
            <ul>
              {#each checks as ck, i (`ck:${i}:${ck.name}`)}
                <li>
                  <a class="row" href={ck.url ?? '#'} target="_blank" rel="noreferrer">
                    <span class="cdot {checkClass(ck.status)}"></span>
                    <span class="title">{ck.name}</span>
                    <span class="cstatus {checkClass(ck.status)}">{ck.status}</span>
                  </a>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <div class="newpr">
          <input class="pr-title" type="text" placeholder="New PR title…" bind:value={prTitle} disabled={creating} />
          <input class="pr-base" type="text" placeholder="base" bind:value={prBase} disabled={creating} />
          <button type="button" class="primary" onclick={createPr} disabled={creating || !prTitle.trim()}>
            {creating ? 'Opening…' : 'New PR'}
          </button>
        </div>
        {#if createdUrl}
          <a class="created mono" href={createdUrl} target="_blank" rel="noreferrer">Opened → {createdUrl}</a>
        {/if}
      {/if}
    {/if}
  </div>
</details>

<style>
  .forge-section { border-top: 1px solid var(--border); }
  .forge-section > summary {
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
  .forge-section > summary::-webkit-details-marker { display: none; }
  .forge-section > summary:hover { color: var(--fg); }
  .forge-section[open] > summary .caret { transform: rotate(90deg); }
  .caret { display: inline-block; transition: transform 120ms ease; color: var(--fg-3); font-size: 10px; }
  .label { color: var(--fg); }

  .body { padding: 8px 16px 14px; display: flex; flex-direction: column; gap: 10px; }

  .err {
    padding: 8px 10px;
    border: 1px solid rgba(255,85,85,0.45);
    background: rgba(255,85,85,0.07);
    color: var(--crash);
    border-radius: var(--radius);
    font-size: 11px;
  }

  .repo { font-size: 11px; color: var(--fg-2); }
  .repo .branch { color: var(--link); }

  .connect .hint { font-size: 11px; color: var(--fg-3); margin: 0 0 6px; }
  .row-form, .newpr { display: flex; gap: 8px; align-items: center; }
  input {
    flex: 1;
    height: 28px;
    padding: 0 10px;
    background: var(--bg);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 11.5px;
    min-width: 0;
  }
  input:disabled { opacity: 0.5; cursor: not-allowed; }
  .newpr .pr-base { flex: 0 0 84px; }

  button.primary {
    background: var(--bg-chrome);
    border: 1px solid var(--border-2);
    color: var(--fg);
    padding: 0 12px;
    height: 28px;
    border-radius: var(--radius-md);
    font-size: 11.5px;
    cursor: pointer;
    white-space: nowrap;
  }
  button.primary:hover:not(:disabled) { border-color: var(--fg-3); }
  button.primary:disabled { opacity: 0.5; cursor: not-allowed; }

  .group header {
    display: flex; align-items: center; gap: 6px;
    font-size: 10.5px; text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--fg-3); margin-bottom: 4px;
  }
  .group .ct { margin-left: auto; font-variant-numeric: tabular-nums; color: var(--fg-2); }
  .group .empty, .empty { font-size: 11px; color: var(--fg-3); padding-left: 2px; }
  .group ul { list-style: none; margin: 0; padding: 0; }
  .group li { display: flex; padding: 2px 0; }

  .row {
    flex: 1; display: flex; align-items: center; gap: 8px;
    background: transparent; border: 0; padding: 3px 6px; border-radius: 3px;
    color: var(--fg); cursor: pointer; text-align: left; min-width: 0;
    text-decoration: none;
  }
  .row:hover { background: var(--bg-tb-hover); }
  .num { color: var(--fg-3); font-size: 10.5px; flex: 0 0 auto; }
  .title { font-size: 11.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tag {
    margin-left: auto; font-size: 9.5px; text-transform: uppercase;
    color: var(--amber); border: 1px solid var(--border-2); border-radius: 3px; padding: 0 4px;
  }

  .cdot { width: 7px; height: 7px; border-radius: 50%; flex: 0 0 auto; }
  .cdot.ok { background: var(--green); }
  .cdot.fail { background: var(--crash); }
  .cdot.pending { background: var(--amber); }
  .cstatus { margin-left: auto; font-size: 10px; }
  .cstatus.ok { color: var(--green); }
  .cstatus.fail { color: var(--crash); }
  .cstatus.pending { color: var(--amber); }

  .created { font-size: 10.5px; color: var(--link); word-break: break-all; }

  @media (max-width: 540px) {
    .row-form, .newpr { flex-wrap: wrap; }
  }
</style>
