<script lang="ts">
  import { api, type NewSession } from '$lib/api';
  import { loadSessions } from '$stores/sessions';
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
  let submitting = $state(false);
  let error = $state<string | null>(null);

  type Tool = {
    id: string;
    label: string;
    desc: string;
    dot: string;
    yoloable: boolean;
  };

  // Tool palette — must match `YOLO_TOOLS` in
  // crates/agentum/src/commands/terminal/app.rs and the executor
  // adapters. The on-the-wire YOLO marker is always
  // --dangerously-skip-permissions; the server translates per-tool
  // (codex: --dangerously-bypass-approvals-and-sandbox, gemini: --yolo).
  const TOOLS: Tool[] = [
    { id: 'claude',   label: 'Claude',   desc: 'Anthropic',    dot: 'var(--tool-claude)', yoloable: true  },
    { id: 'codex',    label: 'Codex',    desc: 'OpenAI',       dot: 'var(--tool-codex)',  yoloable: true  },
    { id: 'gemini',   label: 'Gemini',   desc: 'Google',       dot: 'var(--tool-gemini)', yoloable: true  },
    { id: 'opencode', label: 'opencode', desc: 'open-source',  dot: 'var(--amber)',       yoloable: false },
    { id: 'aider',    label: 'aider',    desc: 'aider.chat',   dot: 'var(--magenta)',     yoloable: false },
    { id: 'terminal', label: 'Terminal', desc: 'plain shell',  dot: 'var(--fg-3)',        yoloable: false },
    { id: 'bash',     label: 'bash',     desc: 'plain shell',  dot: 'var(--fg-3)',        yoloable: false }
  ];

  const currentTool = $derived(TOOLS.find(t => t.id === tool) ?? null);
  const isYoloable  = $derived(currentTool?.yoloable === true);

  function reset() {
    name = '';
    tool = 'claude';
    workdir = '';
    model = '';
    argsRaw = '';
    upAfter = true;
    yolo = false;
    submitting = false;
    error = null;
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
        flags
      };
      const created = await api.createSession(body);
      if (upAfter) {
        try {
          await api.startSession(created.id);
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
        <span class="eyebrow">Agent</span>
        <div class="tools">
          {#each TOOLS as t (t.id)}
            <button
              type="button"
              class="tool"
              class:on={tool === t.id}
              onclick={() => (tool = t.id)}
            >
              <span class="dot" style:background={t.dot}></span>
              <span class="t-name">{t.label}</span>
              <span class="t-desc">{t.desc}</span>
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
            placeholder={tool === 'claude' ? 'claude-opus-4-7' : tool === 'codex' ? 'gpt-5' : 'default'}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      </section>

      <label class="field">
        <span class="lbl">Working directory</span>
        <DirPicker
          bind:value={workdir}
          onChange={(v) => (workdir = v)}
          placeholder="~/projects/foo"
          required
        />
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
  .tool:hover { border-color: var(--fg-3); color: var(--fg); }
  .tool.on {
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 10%, var(--surface));
    color: var(--fg);
  }
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

  @media (max-width: 540px) {
    .tools { grid-template-columns: repeat(2, 1fr); }
    .grid { grid-template-columns: 1fr; }
  }
</style>
