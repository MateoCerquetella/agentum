<script lang="ts">
  import { onMount } from 'svelte';
  import { authState, refreshAuth, login, register } from '$stores/auth';

  interface Props { children: import('svelte').Snippet }
  let { children }: Props = $props();

  let username = $state('');
  let password = $state('');
  let confirm = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);
  let demoMode = $state(false);

  let mode = $derived<'login' | 'register'>(
    $authState === 'needs-setup' ? 'register' : 'login'
  );

  onMount(refreshAuth);

  async function submit(e: Event) {
    e.preventDefault();
    if (!username.trim() || !password) return;
    if (mode === 'register' && password !== confirm) {
      error = 'passwords do not match';
      return;
    }
    submitting = true;
    error = null;
    try {
      if (mode === 'register') {
        await register(username.trim(), password);
      } else {
        await login(username.trim(), password);
      }
      username = '';
      password = '';
      confirm = '';
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
      <div class="logo-area">
        <span class="logo-icon">⟁</span>
      </div>
      <h2>agentum</h2>
      <p class="muted">Self-hosted control plane for AI coding agents.</p>
      <p class="muted warn-msg">Backend not detected — browsing in demo mode.</p>
      <button type="button" class="primary" onclick={() => (demoMode = true)}>
        Explore the dashboard →
      </button>
      <p class="hint muted">
        Run <code>agentum serve</code> on this host to connect a real backend.
      </p>
    </div>
  </div>
{:else if demoMode || $authState === 'ok'}
  {@render children()}
{:else if $authState === 'needs-setup' || $authState === 'needs-login'}
  <div class="full-screen">
    <form class="card" onsubmit={submit}>
      <h2>{mode === 'register' ? 'create your account' : 'log in to agentum'}</h2>
      {#if mode === 'register'}
        <p class="muted">
          No users exist yet — this first registration becomes the admin
          account. After this, the dashboard requires login.
        </p>
      {:else}
        <p class="muted">Sign in with your agentum username and password.</p>
      {/if}

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
          placeholder={mode === 'register' ? 'min 8 characters' : ''}
          autocomplete={mode === 'register' ? 'new-password' : 'current-password'}
          required
          minlength={mode === 'register' ? 8 : undefined}
        />
      </label>

      {#if mode === 'register'}
        <label>
          <span>Confirm password</span>
          <input
            type="password"
            bind:value={confirm}
            autocomplete="new-password"
            required
            minlength={8}
          />
        </label>
      {/if}

      {#if error}<p class="err-msg">{error}</p>{/if}

      <button type="submit" class="primary" disabled={!username.trim() || !password || submitting}>
        {submitting ? (mode === 'register' ? 'creating…' : 'signing in…') : (mode === 'register' ? 'Create account' : 'Sign in')}
      </button>

      {#if mode === 'login'}
        <p class="hint muted">
          Forgot password? Reset all auth on the host with
          <code>agentum auth reset</code>.
        </p>
      {/if}
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
  .full-screen.err { color: var(--danger); }
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
  .warn-msg {
    padding: 0.5rem 0.8rem;
    background: color-mix(in srgb, var(--warn) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--warn) 35%, var(--border));
    border-radius: 6px;
    color: var(--warn);
  }
  .logo-area {
    display: flex;
    justify-content: center;
    margin-bottom: 0.25rem;
  }
  .logo-icon {
    font-size: 2.4rem;
    color: var(--accent);
    font-family: var(--font-mono);
    line-height: 1;
  }
</style>
