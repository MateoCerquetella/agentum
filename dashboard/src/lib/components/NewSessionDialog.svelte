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

  const tools = ['claude', 'codex', 'opencode', 'aider', 'terminal', 'bash'];
  // Must match `YOLO_TOOLS` in `crates/agentum/src/commands/terminal/app.rs`
  // and the set of executor adapters whose `yolo_flag()` returns Some.
  // The on-the-wire flag is always `--dangerously-skip-permissions`
  // (the marker); the server's adapter translates to the per-tool
  // spelling at launch (claude: identity, codex:
  // `--dangerously-bypass-approvals-and-sandbox`, gemini: `--yolo`).
  // `opencode` was here in <=0.6.23 under the wrong assumption that it
  // accepts Claude's flag verbatim — it doesn't, and codex sessions
  // crashed on launch with "unexpected argument" until v0.6.24.
  const yoloTools = new Set(['claude', 'codex', 'gemini']);

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

  // Parse `--arg key=value` lines OR `key=value` pairs (one per line or
  // whitespace-separated) into the flag list expected by the API.
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
      // Drop the trailing slash that the picker adds while drilling in,
      // so backend storage shows a canonical path. `/` itself stays as-is.
      const cleanWorkdir = (() => {
        const w = workdir.trim();
        return w.length > 1 && w.endsWith('/') ? w.replace(/\/+$/, '') : w;
      })();
      const flags = parseArgs(argsRaw);
      if (yolo && yoloTools.has(tool.trim())) {
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
          // Created but couldn't start — surface the error and bail.
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
</script>

<svelte:window onkeydown={open ? onKey : undefined} />

{#if open}
  <div class="backdrop" onmousedown={onBackdrop} role="presentation">
    <form class="dialog" onsubmit={submit}>
      <header>
        <h3>New session</h3>
        <button type="button" class="x" onclick={close} aria-label="close">×</button>
      </header>

      <label>
        <span>Name</span>
        <input
          type="text"
          bind:value={name}
          placeholder="alpha"
          autocomplete="off"
          spellcheck="false"
          required
          autofocus
        />
      </label>

      <div class="row">
        <label class="grow">
          <span>Tool</span>
          <input
            type="text"
            bind:value={tool}
            placeholder="claude"
            list="tool-suggestions"
            autocomplete="off"
            spellcheck="false"
            required
          />
          <datalist id="tool-suggestions">
            {#each tools as t (t)}
              <option value={t}></option>
            {/each}
          </datalist>
        </label>
        <label class="grow">
          <span>Model <small>(optional)</small></span>
          <input
            type="text"
            bind:value={model}
            placeholder="e.g. claude-opus-4-7"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      </div>

      <label>
        <span>Working directory</span>
        <DirPicker
          bind:value={workdir}
          onChange={(v) => (workdir = v)}
          placeholder="~/projects/foo"
          required
        />
      </label>

      <label>
        <span>Extra args <small>(optional, e.g. <code>resume=true model=sonnet</code>)</small></span>
        <input
          type="text"
          bind:value={argsRaw}
          placeholder="key=value pairs, space-separated"
          autocomplete="off"
          spellcheck="false"
        />
      </label>

      <label class="checkbox">
        <input type="checkbox" bind:checked={upAfter} />
        <span>Start immediately (<code>--up</code>)</span>
      </label>

      <label class="checkbox">
        <input type="checkbox" bind:checked={yolo} />
        <span>YOLO mode (<code>--dangerously-skip-permissions</code>)</span>
      </label>

      {#if error}
        <div class="error">{error}</div>
      {/if}

      <footer>
        <button type="button" class="ghost" onclick={close} disabled={submitting}>Cancel</button>
        <button type="submit" class="primary" disabled={submitting}>
          {submitting ? (upAfter ? 'creating + starting…' : 'creating…') : (upAfter ? 'Create + start' : 'Create')}
        </button>
      </footer>
    </form>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    z-index: 80;
    display: grid;
    place-items: center;
    padding: 1rem;
    backdrop-filter: blur(2px);
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1.2rem 1.3rem 1rem;
    width: min(540px, 100%);
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.2rem;
  }
  h3 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1.05rem;
  }
  .x {
    background: none;
    border: none;
    color: var(--muted);
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
    padding: 0 0.4rem;
  }
  .x:hover { color: var(--text); }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.85rem;
    color: var(--text-2);
  }
  label small { color: var(--muted); font-weight: normal; }
  label code {
    font-family: var(--font-mono);
    font-size: 0.78rem;
    color: var(--accent);
    background: var(--surface);
    padding: 0 0.25rem;
    border-radius: 3px;
  }
  input[type='text'] {
    padding: 0.5rem 0.7rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  input[type='text']:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }

  .row { display: flex; gap: 0.7rem; }
  .row .grow { flex: 1; }

  .checkbox {
    flex-direction: row;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
  }
  .checkbox input { accent-color: var(--accent); }

  .error {
    padding: 0.55rem 0.75rem;
    border: 1px solid var(--danger);
    border-radius: 6px;
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-size: 0.82rem;
    font-family: var(--font-mono);
    word-break: break-word;
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.4rem;
  }
  footer button {
    padding: 0.5rem 1rem;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 0.85rem;
    cursor: pointer;
  }
  footer .ghost:hover:not(:disabled) {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  footer .primary {
    background: var(--accent);
    color: var(--bg);
    border-color: var(--accent);
  }
  footer button:disabled { opacity: 0.55; cursor: not-allowed; }
</style>
