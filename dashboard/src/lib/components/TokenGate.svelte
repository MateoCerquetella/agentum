<script lang="ts">
  import { onMount } from 'svelte';
  import { authState, refreshAuth, login } from '$stores/auth';
  import OnboardingWizard from './OnboardingWizard.svelte';
  import {
    profiles,
    activeProfileId,
    upsertProfile,
    setActiveProfile
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

  onMount(refreshAuth);

  // What the unreachable card shows as the failed target. Empty base
  // URL means "this server" (current origin), which is the most
  // common cause when there's nothing serving the SPA's API yet.
  const activeBase = $derived(
    ($profiles.find((p) => p.id === $activeProfileId) ?? $profiles[0])?.baseUrl ||
      (typeof location !== 'undefined' ? location.origin : 'this server')
  );

  function submitAddEndpoint(e: SubmitEvent) {
    e.preventDefault();
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
    try {
      new URL(url);
    } catch {
      formError = 'invalid URL';
      return;
    }
    try {
      upsertProfile({ id, label, baseUrl: url, token: '' });
    } catch (e) {
      formError = e instanceof Error ? e.message : String(e);
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
          refresh, or point this dashboard at a remote one.
        </p>
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
            negotiated per endpoint after you sign in.
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

      <a href="/" class="back-link mono">← back to landing page</a>
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
  .back-link {
    display: inline-block;
    margin-top: 0.5rem;
    font-size: 0.82rem;
    color: var(--accent);
    text-decoration: none;
  }
  .back-link:hover { text-decoration: underline; }

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
