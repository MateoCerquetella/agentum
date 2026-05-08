<script lang="ts">
  import { onMount } from 'svelte';
  import { authState, refreshAuth, login } from '$stores/auth';
  import OnboardingWizard from './OnboardingWizard.svelte';
  import {
    profiles,
    activeProfileId,
    upsertProfile,
    setActiveProfile,
    removeProfile
  } from '$lib/profiles';

  interface Props { children: import('svelte').Snippet }
  let { children }: Props = $props();

  let username = $state('');
  let password = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);

  // Inline "add endpoint" form shown on the unreachable card so the
  // user can recover without leaving the page. Mirrors the TUI's
  // empty-daemon prompt and the EndpointSwitcher's add form.
  let showAddForm = $state(false);
  let formId = $state('');
  let formLabel = $state('');
  let formUrl = $state('');
  let formError = $state<string | null>(null);
  let formChecking = $state(false);

  onMount(refreshAuth);

  // What the unreachable card shows as the failed target. Empty base
  // URL means "this server" (current origin), which is the most
  // common cause when there's nothing serving the SPA's API yet.
  const activeBase = $derived(
    ($profiles.find((p) => p.id === $activeProfileId) ?? $profiles[0])?.baseUrl ||
      (typeof location !== 'undefined' ? location.origin : 'this server')
  );

  // Other configured profiles the user can switch to without leaving
  // the unreachable card. Filters out the currently active one (it's
  // the one that just failed) so the list only shows recovery options.
  const otherProfiles = $derived(
    $profiles.filter((p) => p.id !== $activeProfileId)
  );

  function switchProfile(id: string) {
    setActiveProfile(id);
    if (typeof location !== 'undefined') location.reload();
  }

  function dropProfile(id: string) {
    if (!confirm(`Remove this saved endpoint?`)) return;
    removeProfile(id);
  }

  async function submitAddEndpoint(e: SubmitEvent) {
    e.preventDefault();
    if (formChecking) return;
    formError = null;
    const id = formId.trim();
    const label = formLabel.trim() || id;
    const url = formUrl.trim();
    if (!id) {
      formError = 'id is required';
      return;
    }
    if (!url) {
      formError = 'URL is required';
      return;
    }
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      formError = 'invalid URL';
      return;
    }

    // Health-probe before saving: hit `<url>/api/health` and confirm
    // it actually answers with the agentum daemon shape. Saving an
    // unreachable endpoint just reproduces the unreachable state with
    // a stale entry the user has to clean up later. CORS / mixed-
    // content / wrong-port errors all surface here as a fetch
    // exception or non-OK status, with a hint on the most common
    // mistakes.
    formChecking = true;
    try {
      const probeBase = `${parsed.origin}${parsed.pathname.replace(/\/+$/, '')}`;
      const probeUrl = `${probeBase}/api/health`;
      const ctrl = new AbortController();
      const t = setTimeout(() => ctrl.abort(), 4000);
      let res: Response;
      try {
        res = await fetch(probeUrl, { signal: ctrl.signal, mode: 'cors' });
      } finally {
        clearTimeout(t);
      }
      if (!res.ok) {
        formError = `health check failed: HTTP ${res.status}`;
        formChecking = false;
        return;
      }
      const body: unknown = await res.json().catch(() => null);
      if (
        !body ||
        typeof body !== 'object' ||
        (body as { status?: unknown }).status !== 'ok'
      ) {
        formError = 'health check returned unexpected shape — is this an agentum daemon?';
        formChecking = false;
        return;
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      const hint =
        location.protocol === 'https:' && parsed.protocol === 'http:'
          ? ' (mixed content: this dashboard is served over HTTPS, the endpoint is HTTP — open the dashboard from the daemon directly, or use HTTPS)'
          : msg.toLowerCase().includes('cors') || msg.toLowerCase().includes('failed to fetch')
            ? ' (could be CORS, the daemon is unreachable, or the URL is wrong — the daemon must be running and reachable from this browser)'
            : '';
      formError = `health check failed: ${msg}${hint}`;
      formChecking = false;
      return;
    }

    try {
      upsertProfile({ id, label, baseUrl: url, token: '' });
    } catch (e) {
      formError = e instanceof Error ? e.message : String(e);
      formChecking = false;
      return;
    }
    setActiveProfile(id);
    // Reload so every store, fetch, and WS re-evaluates against the
    // new profile. Same approach as the EndpointSwitcher; the alt is
    // hand-wiring re-init across every store, which is fragile.
    if (typeof location !== 'undefined') location.reload();
  }


  async function submitLogin(e: Event) {
    e.preventDefault();
    if (!username.trim() || !password) return;
    submitting = true;
    error = null;
    try {
      await login(username.trim(), password);
      username = '';
      password = '';
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      submitting = false;
    }
  }
</script>

{#if $authState === 'unknown'}
  <div class="full-screen muted">connecting…</div>
{:else if $authState === 'unreachable'}
  <div class="full-screen">
    <div class="card">
      <h2>backend unreachable</h2>
      <p class="muted">
        No agentum daemon answered at <code>{activeBase}</code>.
      </p>

      {#if !showAddForm}
        <p class="muted">
          Either start a local daemon with <code>agentum serve</code> and
          refresh, or switch to one of your saved endpoints.
        </p>

        {#if otherProfiles.length > 0}
          <div class="endpoints">
            <p class="muted small endpoints-header">switch to a saved endpoint:</p>
            {#each otherProfiles as p (p.id)}
              <div class="endpoint-row">
                <button
                  type="button"
                  class="endpoint-pick"
                  onclick={() => switchProfile(p.id)}
                  title="Switch to {p.label}"
                >
                  <span class="endpoint-label">{p.label}</span>
                  <span class="endpoint-url mono">{p.baseUrl || 'this server'}</span>
                </button>
                <button
                  type="button"
                  class="endpoint-rm"
                  onclick={() => dropProfile(p.id)}
                  title="Remove {p.label}"
                  aria-label="Remove endpoint"
                >×</button>
              </div>
            {/each}
          </div>
        {/if}

        <div class="actions">
          <button type="button" class="primary" onclick={() => (showAddForm = true)}>
            Add a remote endpoint
          </button>
          <button type="button" class="ghost" onclick={() => location.reload()}>
            Retry
          </button>
        </div>
      {:else}
        <form class="add" onsubmit={submitAddEndpoint}>
          <p class="muted small">
            Saved to your browser's local storage. The bearer token is
            negotiated per endpoint after you sign in. For a daemon on
            your own machine, use <code>http://localhost:8822</code>
            (or its LAN IP, e.g. <code>http://192.168.x.x:8822</code>,
            if the browser isn't on the same machine).
          </p>
          <label>
            <span>id</span>
            <input
              type="text"
              bind:value={formId}
              placeholder="vps"
              autocomplete="off"
              spellcheck="false"
              required
            />
          </label>
          <label>
            <span>label <span class="opt">(optional)</span></span>
            <input
              type="text"
              bind:value={formLabel}
              placeholder="My production VPS"
              autocomplete="off"
              spellcheck="false"
            />
          </label>
          <label>
            <span>URL</span>
            <input
              type="url"
              bind:value={formUrl}
              placeholder="https://my-vps.example.com:8822"
              autocomplete="off"
              spellcheck="false"
              required
            />
          </label>
          {#if formError}<p class="err-msg">{formError}</p>{/if}
          <div class="actions">
            <button type="submit" class="primary">Save & connect</button>
            <button type="button" class="ghost" onclick={() => (showAddForm = false)}>
              Cancel
            </button>
          </div>
        </form>
      {/if}
    </div>
  </div>
{:else if $authState === 'needs-setup'}
  <OnboardingWizard />
{:else if $authState === 'needs-login'}
  <div class="full-screen">
    <form class="card" onsubmit={submitLogin}>
      <h2>log in to agentum</h2>
      <p class="muted">Sign in with your agentum username and password.</p>

      <label>
        <span>Username</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          bind:value={username}
          placeholder="username"
          autocomplete="username"
          spellcheck="false"
          required
          autofocus
        />
      </label>

      <label>
        <span>Password</span>
        <input
          type="password"
          bind:value={password}
          autocomplete="current-password"
          required
        />
      </label>

      {#if error}<p class="err-msg">{error}</p>{/if}

      <button type="submit" class="primary" disabled={!username.trim() || !password || submitting}>
        {submitting ? 'signing in…' : 'Sign in'}
      </button>

      <p class="hint muted">
        Forgot password? Reset all auth on the host with
        <code>agentum auth reset</code>.
      </p>
    </form>
  </div>
{:else}
  {@render children()}
{/if}

<style>
  .full-screen {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    width: 100%;
    background: var(--bg);
    padding: 1rem;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1.5rem 1.7rem;
    max-width: 440px;
    width: 100%;
    box-shadow: 0 4px 22px rgba(0,0,0,0.18);
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1.15rem;
    color: var(--text);
  }
  .muted { color: var(--muted); margin: 0; font-size: 0.86rem; line-height: 1.45; }
  code { font-family: var(--font-mono); color: var(--accent); }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.82rem;
    color: var(--text-2);
  }
  input {
    padding: 0.55rem 0.8rem;
    background: var(--surface-2);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.9rem;
  }
  input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .primary {
    background: var(--accent);
    color: var(--bg);
    border: 0;
    border-radius: 6px;
    padding: 0.6rem 1rem;
    font-family: var(--font-mono);
    font-size: 0.9rem;
    cursor: pointer;
    margin-top: 0.2rem;
  }
  .primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .err-msg {
    margin: 0;
    color: var(--danger);
    font-size: 0.85rem;
    font-family: var(--font-mono);
    word-break: break-word;
  }
  .hint {
    margin-top: 0.5rem;
    font-size: 0.75rem;
  }
  .endpoints {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    margin-top: 0.2rem;
  }
  .endpoints-header {
    margin-bottom: 0.1rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-size: 0.72rem;
  }
  .endpoint-row {
    display: flex;
    gap: 0.35rem;
    align-items: stretch;
  }
  .endpoint-pick {
    flex: 1;
    text-align: left;
    padding: 0.5rem 0.7rem;
    background: var(--surface-2, var(--surface));
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    font-family: var(--font-mono);
    font-size: 0.82rem;
    transition: border-color 120ms ease;
  }
  .endpoint-pick:hover {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .endpoint-label { color: var(--text); font-weight: 500; }
  .endpoint-url {
    color: var(--muted);
    font-size: 0.75rem;
    word-break: break-all;
  }
  .endpoint-rm {
    flex-shrink: 0;
    width: 32px;
    background: var(--surface-2, var(--surface));
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 1.1rem;
    line-height: 1;
    cursor: pointer;
    transition: color 120ms ease, border-color 120ms ease;
  }
  .endpoint-rm:hover {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 40%, var(--border));
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.4rem;
  }
  .actions button {
    flex: 1;
    padding: 0.55rem 0.9rem;
    border-radius: 6px;
    border: 1px solid var(--border);
    font-family: var(--font-mono);
    font-size: 0.85rem;
    cursor: pointer;
  }
  .actions .ghost {
    background: var(--surface-2, var(--surface));
    color: var(--text);
  }
  .actions .ghost:hover { border-color: color-mix(in srgb, var(--accent) 40%, var(--border)); }
  .add { display: flex; flex-direction: column; gap: 0.55rem; }
  .add .small { font-size: 0.78rem; }
  .opt { color: var(--muted); font-weight: normal; font-size: 0.78rem; }
</style>
