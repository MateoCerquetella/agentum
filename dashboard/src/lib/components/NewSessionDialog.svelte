<script lang="ts">
  import { api, type NewSession, type AgentInfo, type Host } from '$lib/api';
  import { loadSessions } from '$stores/sessions';
  import { fleet } from '$stores/fleet';
  import { hosts, refreshHosts, hostLabel } from '$stores/hosts';
  import { profiles, activeProfileId, type Profile } from '$lib/profiles';
  import DirPicker from './DirPicker.svelte';

  type Props = { open: boolean; onClose: () => void };
  let { open, onClose }: Props = $props();

  let name = $state('');
  let tool = $state('claude');
  let workdir = $state('');
  let model = $state('');
  let argsRaw = $state('');
  let upAfter = $state(true);
  let yolo = $state(false);
  /// Default-on: create a dedicated `git worktree` for this session so
  /// it runs on its own branch + checkout. Lets the user spawn N agents
  /// against the same repo in parallel without stomping each other's
  /// stash/branch. Toggle off for a non-git workdir; forced off when an
  /// explicit host is picked (worktrees are local-host only for now —
  /// see `pickHost`). Parity with the TUI's worktree-by-default toggle.
  /// See ORCA_COMPETITIVE_ANALYSIS.md §1.
  let useWorktree = $state(true);
  /// Ref the new worktree branch forks from. Empty → server defaults to
  /// `HEAD`. Branch name is always derived from the session name
  /// (server-side slug) — we don't expose a freeform branch input
  /// because the more useful knob is "off what".
  let baseRef = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);
  /// Target server profile id. Mirrors the TUI's `Servers` field on the
  /// New Session form — spawning goes against this profile's daemon
  /// regardless of which one the topbar EndpointSwitcher has "active".
  /// Defaults to the active profile so the most common "spawn here"
  /// flow still works without extra clicks.
  let targetProfileId = $state('');
  let targetHostId = $state('');
  /// Used to no-op the workdir refetch effect when the dialog isn't
  /// open yet — otherwise we'd fire a fetch every time the active
  /// profile changes outside of an open form.
  let lastFetchedFor = $state<string | null>(null);
  /// Set of tool ids whose binary resolves on the daemon's PATH. Empty
  /// while the probe is in flight; populated from `/api/agents` on
  /// dialog open. Tools missing from the map are treated as available
  /// (passthrough) so unknown free-form tool ids stay spawnable.
  let availability = $state<Record<string, AgentInfo>>({});
  let targetHosts = $state<Host[]>([]);

  /// Profiles list rendered as Servers tiles. We don't synthesize a
  /// virtual "this machine" entry the way the TUI does — the dashboard's
  /// `profiles` store already includes the local profile (the default
  /// `{ id: 'local', baseUrl: '' }` entry created on first load),
  /// so it sits naturally at the top of the list. Labels: any profile
  /// with an empty `baseUrl` shows as "this machine" so the matching
  /// label parity with the TUI is preserved.
  const serverTiles = $derived<Profile[]>($profiles);
  function serverLabel(p: Profile): string {
    if (p.baseUrl) return p.label;
    // Loopback: pull the real hostname from the fleet store (populated
    // by /api/health) so users see "omarchy" / "mateo-mac" instead of
    // a generic placeholder. Matches the Sidebar convention.
    const host = $fleet[p.id]?.hostname?.trim();
    return host || 'this machine';
  }
  function serverHost(p: Profile): string {
    if (p.baseUrl) {
      try { return new URL(p.baseUrl).host; } catch { return p.baseUrl; }
    }
    // Loopback: label already carries the hostname; keep the host hint
    // empty so we don't repeat "omarchy / omarchy" or print a misleading
    // hardcoded URL.
    return '';
  }

  type Tool = {
    id: string;
    label: string;
    desc: string;
    dot: string;
    yoloable: boolean;
    /// First-class tool ids appear in `/api/agents` and are gated on
    /// installation. Non-first-class entries (terminal, bash, aider…)
    /// are always shown.
    firstClass: boolean;
  };

  // Tool palette — must match `YOLO_TOOLS` in
  // crates/agentum/src/commands/terminal/app.rs and the executor
  // adapters. The on-the-wire YOLO marker is always
  // --dangerously-skip-permissions; the server translates per-tool
  // (codex: --dangerously-bypass-approvals-and-sandbox, gemini: --yolo,
  // cursor: --force).
  const TOOLS: Tool[] = [
    { id: 'claude',   label: 'Claude',   desc: 'Anthropic',    dot: 'var(--tool-claude)',  yoloable: true,  firstClass: true  },
    { id: 'codex',    label: 'Codex',    desc: 'OpenAI',       dot: 'var(--tool-codex)',   yoloable: true,  firstClass: true  },
    { id: 'cursor',   label: 'Cursor',   desc: 'cursor-agent', dot: 'var(--tool-cursor, var(--cta))', yoloable: true,  firstClass: true  },
    { id: 'agent',    label: 'Agent',    desc: 'Cursor agent', dot: 'var(--tool-cursor, var(--cta))', yoloable: true,  firstClass: true  },
    { id: 'gemini',   label: 'Gemini',   desc: 'Google',       dot: 'var(--tool-gemini)',  yoloable: true,  firstClass: true  },
    { id: 'opencode', label: 'opencode', desc: 'open-source',  dot: 'var(--amber)',        yoloable: false, firstClass: true  },
    { id: 'aider',    label: 'aider',    desc: 'aider.chat',   dot: 'var(--magenta)',      yoloable: false, firstClass: true  },
    { id: 'terminal', label: 'Terminal', desc: 'plain shell',  dot: 'var(--fg-3)',         yoloable: false, firstClass: false },
    { id: 'bash',     label: 'bash',     desc: 'plain shell',  dot: 'var(--fg-3)',         yoloable: false, firstClass: false }
  ];

  const currentTool = $derived(TOOLS.find(t => t.id === tool) ?? null);
  const isYoloable  = $derived(currentTool?.yoloable === true);

  /// First-class tools missing from PATH are disabled (no spawn) with a
  /// tooltip pointing at the missing binary. Non-first-class tools and
  /// any tool whose probe hasn't returned yet stay enabled.
  function toolAvailable(t: Tool): boolean {
    if (!t.firstClass) return true;
    const info = availability[t.id];
    if (!info) return true; // probe pending / endpoint absent
    return info.available;
  }

  function toolUnavailableReason(t: Tool): string | null {
    if (!t.firstClass) return null;
    const info = availability[t.id];
    if (!info || info.available) return null;
    return `${info.binary} not found on the daemon's PATH`;
  }

  async function refreshAvailability() {
    const profileId = targetProfileId;
    const hostId = targetHostId || null;
    try {
      const list = await api.listAgentsOn(profileId, hostId);
      if (profileId !== targetProfileId || hostId !== (targetHostId || null)) return;
      const map: Record<string, AgentInfo> = {};
      for (const a of list) map[a.name] = a;
      availability = map;
    } catch {
      if (profileId !== targetProfileId || hostId !== (targetHostId || null)) return;
      // Older daemons (pre-this-change) won't expose /api/agents. Fail
      // open: the picker shows everything and a missing binary surfaces
      // as a `command not found` later instead of being blocked here.
      availability = {};
    }
  }

  async function refreshTargetHosts() {
    const profileId = targetProfileId;
    try {
      const list = await api.listHostsOn(profileId);
      if (profileId !== targetProfileId) return;
      targetHosts = list;
      if (targetHostId && !targetHosts.some((h) => h.id === targetHostId)) {
        targetHostId = '';
      }
    } catch {
      if (profileId !== targetProfileId) return;
      targetHosts = [];
      targetHostId = '';
    }
  }

  /// Pre-fill the workdir with the *target* daemon's `$HOME` so the
  /// user doesn't end up typing a path that exists on the laptop but
  /// not on the chosen server (the same trap the TUI's Servers cycle
  /// has been fixing across v0.7.13–v0.7.15). Only fires when the
  /// target profile changes since the last fetch — otherwise editing
  /// the workdir manually and then refocusing the dialog would clobber
  /// the user's typed path.
  async function refreshWorkdirHome() {
    const profileId = targetProfileId;
    const hostId = targetHostId || null;
    const fetchKey = `${profileId}:${hostId || 'local'}`;
    if (lastFetchedFor === fetchKey) return;
    try {
      const listing = await api.listDirHostOn(profileId, hostId, undefined);
      if (profileId !== targetProfileId || hostId !== (targetHostId || null)) return;
      workdir = listing.path;
      lastFetchedFor = fetchKey;
      // A successful refetch means the target is reachable; clear any
      // stale per-target error so the user isn't staring at a message
      // that no longer applies.
      if (error && error.startsWith("couldn't reach ")) error = null;
    } catch (e) {
      if (profileId !== targetProfileId || hostId !== (targetHostId || null)) return;
      const label = (() => {
        const p = $profiles.find((x) => x.id === targetProfileId);
        return p ? serverLabel(p) : targetProfileId;
      })();
      error = `couldn't reach ${label}: ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  $effect(() => {
    if (open) {
      // Seed the target profile from the active one each time the
      // dialog opens. The user can switch via the Servers tiles
      // afterwards; we don't want the field to be sticky across
      // opens or the topbar's EndpointSwitcher would feel like it
      // wasn't doing anything when re-opening the dialog.
      if (!targetProfileId) targetProfileId = $activeProfileId;
      if (!$hosts.length) void refreshHosts();
      void refreshTargetHosts();
      void refreshAvailability();
      void refreshWorkdirHome();
    }
  });

  /// When the user picks a different server in the dialog, re-probe
  /// availability (the new server may have different agents installed)
  /// and refetch its `$HOME` for the workdir field.
  async function pickServer(id: string) {
    if (id === targetProfileId) return;
    targetProfileId = id;
    targetHostId = '';
    targetHosts = [];
    lastFetchedFor = null;
    await Promise.all([refreshTargetHosts(), refreshAvailability(), refreshWorkdirHome()]);
  }

  async function pickHost(id: string) {
    if (id === targetHostId) return;
    targetHostId = id;
    useWorktree = false;
    availability = {};
    lastFetchedFor = null;
    await Promise.all([refreshAvailability(), refreshWorkdirHome()]);
  }

  // If the user had selected a tool that just became unavailable
  // (rare — happens if the daemon's PATH changes between dialog opens),
  // bounce them back to the first available first-class option.
  $effect(() => {
    const t = TOOLS.find(x => x.id === tool);
    if (t && t.firstClass && !toolAvailable(t)) {
      const fallback = TOOLS.find(x => x.firstClass && toolAvailable(x));
      if (fallback) tool = fallback.id;
    }
  });

  function reset() {
    name = '';
    tool = 'claude';
    workdir = '';
    model = '';
    argsRaw = '';
    upAfter = true;
    yolo = false;
    useWorktree = true;
    baseRef = '';
    submitting = false;
    error = null;
    targetProfileId = '';
    targetHostId = '';
    lastFetchedFor = null;
  }

  function close() {
    reset();
    onClose();
  }

  function parseArgs(input: string): string[] {
    const out: string[] = [];
    for (const tok of input.split(/\s+/).filter(Boolean)) {
      const eq = tok.indexOf('=');
      if (eq < 0) continue;
      const k = tok.slice(0, eq).replace(/^--/, '');
      const v = tok.slice(eq + 1);
      out.push(v === 'true' ? `--${k}` : `--${k}=${v}`);
    }
    return out;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!name.trim() || !workdir.trim() || !tool.trim()) {
      error = 'name, tool, and workdir are required';
      return;
    }
    submitting = true;
    error = null;
    try {
      const cleanWorkdir = (() => {
        const w = workdir.trim();
        return w.length > 1 && w.endsWith('/') ? w.replace(/\/+$/, '') : w;
      })();
      const flags = parseArgs(argsRaw);
      if (yolo && isYoloable) {
        flags.push('--dangerously-skip-permissions');
      }
      const body: NewSession = {
        name: name.trim(),
        tool: tool.trim(),
        workdir: cleanWorkdir,
        model: model.trim() || null,
        flags,
        ...(targetHostId ? { host_id: targetHostId } : {}),
        // Only attach the worktree key when the toggle is on — leaving
        // it undefined keeps the JSON wire payload identical to the
        // pre-worktree shape, so an older daemon (no `CreateBody.worktree`)
        // still accepts the request.
        ...(useWorktree
          ? { worktree: { base_ref: baseRef.trim() || undefined } }
          : {})
      };
      // Spawn against the *target* server picked by the Servers tiles,
      // not just whatever the topbar EndpointSwitcher has active.
      // Empty target degrades to the active profile inside
      // `createSessionOn`, so existing single-server flows still work.
      const created = await api.createSessionOn(targetProfileId, body);
      if (upAfter) {
        try {
          await api.startSessionOn(targetProfileId, created.id);
        } catch (startErr) {
          error = startErr instanceof Error ? startErr.message : String(startErr);
          submitting = false;
          await loadSessions();
          return;
        }
      }
      await loadSessions();
      close();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      submitting = false;
    }
  }

  function onBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget && !submitting) close();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && !submitting) close();
  }

  // If the user picks a non-yoloable tool while YOLO is on, drop the
  // toggle so we don't ship a flag the adapter will reject.
  $effect(() => {
    if (!isYoloable && yolo) yolo = false;
  });
</script>

<svelte:window onkeydown={open ? onKey : undefined} />

{#if open}
  <div class="backdrop" onmousedown={onBackdrop} role="presentation">
    <form class="dialog" onsubmit={submit}>
      <header>
        <div>
          <h3>Spawn session</h3>
          <p class="sub">A new tmux pane, wired to the agent of your choice.</p>
        </div>
        <button type="button" class="x" onclick={close} aria-label="close">×</button>
      </header>

      <section>
        <span class="eyebrow">Servers</span>
        <div class="tools servers">
          {#each serverTiles as p (p.id)}
            <button
              type="button"
              class="tool"
              class:on={targetProfileId === p.id}
              onclick={() => pickServer(p.id)}
              title={p.baseUrl || 'http://current-origin'}
            >
              <span class="dot" style:background={p.baseUrl ? 'var(--cta)' : 'var(--green, #2ea043)'}></span>
              <span class="t-name">{serverLabel(p)}</span>
              <span class="t-desc">{serverHost(p)}</span>
            </button>
          {/each}
        </div>
      </section>

      <section>
        <span class="eyebrow">Hosts</span>
        <div class="tools servers">
          {#each targetHosts as h (h.id)}
            <button
              type="button"
              class="tool"
              class:on={(targetHostId || '00000000-0000-0000-0000-000000000000') === h.id}
              onclick={() => pickHost(h.kind === 'local' ? '' : h.id)}
              title={h.kind === 'local' ? 'this machine' : `${h.user}@${h.hostname}:${h.port}`}
            >
              <span class="dot" style:background={h.kind === 'local' ? 'var(--green, #2ea043)' : 'var(--cta)'}></span>
              <span class="t-name">{h.name}</span>
              <span class="t-desc">{hostLabel(h)}</span>
            </button>
          {/each}
        </div>
      </section>

      <section>
        <span class="eyebrow">Agent</span>
        <div class="tools">
          {#each TOOLS as t (t.id)}
            {@const avail = toolAvailable(t)}
            {@const reason = toolUnavailableReason(t)}
            <button
              type="button"
              class="tool"
              class:on={tool === t.id}
              class:off={!avail}
              disabled={!avail}
              title={reason ?? ''}
              onclick={() => avail && (tool = t.id)}
            >
              <span class="dot" style:background={t.dot}></span>
              <span class="t-name">{t.label}</span>
              <span class="t-desc">{avail ? t.desc : 'not installed'}</span>
            </button>
          {/each}
        </div>
      </section>

      <section class="grid">
        <label class="field">
          <span class="lbl">Name</span>
          <input
            type="text"
            bind:value={name}
            placeholder="alpha"
            autocomplete="off"
            spellcheck="false"
            required
          />
        </label>
        <label class="field">
          <span class="lbl">Model <span class="opt">optional</span></span>
          <input
            type="text"
            bind:value={model}
            placeholder={tool === 'claude' ? 'claude-opus-4-8' : tool === 'codex' ? 'gpt-5' : 'default'}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      </section>

      <label class="field">
        <span class="lbl">Working directory</span>
        {#key `${targetProfileId}:${targetHostId || 'local'}`}
          <DirPicker
            bind:value={workdir}
            onChange={(v) => (workdir = v)}
            profileId={targetProfileId}
            hostId={targetHostId || null}
            placeholder="~/projects/foo"
            required
          />
        {/key}
      </label>

      <section class="toggles">
        <label class="toggle">
          <input type="checkbox" bind:checked={upAfter} />
          <span class="t-text">
            <span class="t-title">Start immediately</span>
            <span class="t-sub">spawn the pane and bring the agent up after creating</span>
          </span>
        </label>
        <label class="toggle" class:disabled={!isYoloable}>
          <input type="checkbox" bind:checked={yolo} disabled={!isYoloable} />
          <span class="t-text">
            <span class="t-title danger">YOLO mode</span>
            <span class="t-sub">
              {isYoloable
                ? 'skip permission prompts (--dangerously-skip-permissions)'
                : `not supported by ${currentTool?.label ?? tool}`}
            </span>
          </span>
        </label>
        <label class="toggle" class:disabled={!!targetHostId}>
          <input type="checkbox" bind:checked={useWorktree} disabled={!!targetHostId} />
          <span class="t-text">
            <span class="t-title">Isolate in git worktree</span>
            <span class="t-sub">
              spawn on a fresh branch + checkout at
              <code>&lt;repo&gt;-worktrees/agentum-&lt;name&gt;</code>
              — run N agents on the same repo without collisions
            </span>
          </span>
        </label>
        {#if useWorktree}
          <label class="field" style="margin-left: 28px; margin-top: -4px;">
            <span class="lbl">Base ref <span class="opt">defaults to HEAD</span></span>
            <input
              type="text"
              bind:value={baseRef}
              placeholder="HEAD"
              autocomplete="off"
              spellcheck="false"
            />
          </label>
        {/if}
      </section>

      <details>
        <summary>Advanced</summary>
        <label class="field" style="margin-top: 10px;">
          <span class="lbl">Extra flags <span class="opt">key=value, space-separated</span></span>
          <input
            type="text"
            bind:value={argsRaw}
            placeholder="resume=true profile=opus"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      </details>

      {#if error}
        <div class="error">{error}</div>
      {/if}

      <footer>
        <button type="button" class="ghost" onclick={close} disabled={submitting}>Cancel</button>
        <button type="submit" class="primary" disabled={submitting}>
          {#if submitting}
            <span class="spin"></span>
            {upAfter ? 'creating + starting…' : 'creating…'}
          {:else}
            <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
              <path d="M3 8h10M8 3v10" stroke-linecap="round"/>
            </svg>
            {upAfter ? 'Spawn + start' : 'Spawn'}
          {/if}
        </button>
      </footer>
    </form>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.62);
    z-index: 80;
    display: grid;
    place-items: center;
    padding: 1rem;
    backdrop-filter: blur(3px);
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-lg);
    padding: 22px 22px 16px;
    width: min(620px, 100%);
    max-height: 90vh;
    overflow-y: auto;
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.02) inset,
      0 24px 64px rgba(0, 0, 0, 0.55);
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-family: var(--display);
    font-size: 18px;
    font-weight: 500;
    letter-spacing: -0.02em;
    color: var(--fg);
  }
  .sub {
    margin: 4px 0 0;
    color: var(--fg-3);
    font-size: 12.5px;
  }
  .x {
    background: none;
    border: 0;
    color: var(--fg-3);
    font-size: 22px;
    line-height: 1;
    cursor: pointer;
    padding: 0 6px;
    transition: color var(--t-hover);
  }
  .x:hover { color: var(--fg); }

  section { display: flex; flex-direction: column; gap: 8px; }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }

  .eyebrow {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
  }

  .tools {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 6px;
  }
  .tool {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
    padding: 9px 10px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg-2);
    cursor: pointer;
    transition: border-color var(--t-hover), background var(--t-hover), color var(--t-hover);
    text-align: left;
  }
  .tool:hover:not(:disabled) { border-color: var(--fg-3); color: var(--fg); }
  .tool.on {
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 10%, var(--surface));
    color: var(--fg);
  }
  .tool.off {
    opacity: 0.4;
    cursor: not-allowed;
    /* Strike through the dot so the "missing binary" state reads even
       without hovering for the tooltip. */
    filter: grayscale(0.6);
  }
  .tool.off .t-desc { color: var(--crash); }
  .tool .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill);
  }
  .tool .t-name {
    font-size: 13px;
    letter-spacing: -0.01em;
    color: inherit;
  }
  .tool .t-desc {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-3);
  }

  .field { display: flex; flex-direction: column; gap: 6px; }
  .lbl {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
  }
  .opt { color: var(--fg-3); text-transform: none; letter-spacing: 0; font-size: 10px; }

  input[type='text'] {
    padding: 8px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 12.5px;
    transition: border-color var(--t-hover);
  }
  input[type='text']:focus {
    outline: none;
    border-color: var(--cta);
  }
  input[type='text']::placeholder { color: var(--fg-3); }

  .toggles {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .toggle {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 10px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: border-color var(--t-hover);
  }
  .toggle:hover { border-color: var(--fg-3); }
  .toggle.disabled { opacity: 0.55; cursor: not-allowed; }
  .toggle input { accent-color: var(--cta); margin-top: 2px; }
  .t-text { display: flex; flex-direction: column; gap: 2px; }
  .t-title { font-size: 12.5px; color: var(--fg); letter-spacing: -0.005em; }
  .t-title.danger { color: var(--cta); }
  .t-sub {
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--fg-3);
  }

  details > summary {
    cursor: pointer;
    color: var(--fg-2);
    font-family: var(--mono);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    list-style: none;
    user-select: none;
    padding: 4px 0;
  }
  details > summary::-webkit-details-marker { display: none; }
  details > summary::before {
    content: '▸';
    display: inline-block;
    margin-right: 6px;
    color: var(--fg-3);
    transition: transform var(--t-transform);
  }
  details[open] > summary::before { transform: rotate(90deg); }

  .error {
    padding: 8px 10px;
    border: 1px solid var(--crash);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--crash) 10%, transparent);
    color: var(--crash);
    font-size: 12px;
    font-family: var(--mono);
    word-break: break-word;
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }
  footer button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 7px 14px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border-2);
    background: var(--surface);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 11.5px;
    cursor: pointer;
    transition: border-color var(--t-hover), background var(--t-hover), color var(--t-hover);
  }
  footer .ghost:hover:not(:disabled) {
    color: var(--fg);
    border-color: var(--fg-3);
  }
  footer .primary {
    background: var(--cta);
    color: #fff;
    border-color: var(--cta);
  }
  footer .primary:hover:not(:disabled) {
    filter: brightness(1.05);
  }
  footer button:disabled { opacity: 0.55; cursor: not-allowed; }

  .spin {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 1.5px solid rgba(255,255,255,0.45);
    border-top-color: #fff;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  @media (max-width: 720px) {
    /* Bottom-sheet treatment on phones — slides up from below, full
       width, rounded only on the top corners. Reads as native modal. */
    .backdrop {
      align-items: flex-end;
      padding: 0;
    }
    .dialog {
      width: 100%;
      max-width: 100%;
      max-height: 92dvh;
      border-radius: 18px 18px 0 0;
      padding: 18px 16px calc(18px + env(safe-area-inset-bottom, 0px));
      animation: sheet-in 220ms cubic-bezier(0.2, 0.7, 0.2, 1);
    }
    @keyframes sheet-in {
      from { transform: translateY(100%); }
      to   { transform: translateY(0); }
    }
    .x { font-size: 26px; padding: 4px 10px; }
    .tool { padding: 12px 12px; }
    .tool .t-name { font-size: 14px; }
    input[type='text'] {
      padding: 12px 12px;
      font-size: 16px !important;
      border-radius: 10px;
    }
    .toggle { padding: 12px; }
    footer {
      flex-direction: column-reverse;
      gap: 10px;
      padding-top: 14px;
    }
    footer button {
      width: 100%;
      justify-content: center;
      padding: 12px 16px;
      font-size: 13px;
      border-radius: 10px;
      min-height: 44px;
    }
  }
  @media (max-width: 540px) {
    .tools { grid-template-columns: repeat(2, 1fr); }
    .grid { grid-template-columns: 1fr; }
  }
</style>
