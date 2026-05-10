<script lang="ts">
  /**
   * First-run onboarding wizard. Runs when `/api/auth/status` reports
   * `needs_setup: true`. Four steps:
   *
   *   1. Welcome — what is agentum, set expectations.
   *   2. Create admin — username + password.
   *   3. Verify cert — show the SHA-256 fingerprint, ask the user to
   *      confirm it matches what the host printed. Skipped on --no-tls.
   *   4. Share access — display the dashboard URL + fingerprint with
   *      copy buttons, plus a 3-line cheat sheet for LAN / public IP /
   *      VPN modes.
   *
   * After "Done" the user lands on the sessions page with a fresh
   * bearer token.
   */
  import { register } from '$stores/auth';
  import { api } from '$lib/api';
  import RemoteAccessInfo from './RemoteAccessInfo.svelte';

  type Step = 'welcome' | 'register' | 'verify' | 'share' | 'done';

  let step = $state<Step>('welcome');
  let username = $state('');
  let password = $state('');
  let confirm = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);
  let tlsActive = $state<boolean | null>(null);

  $effect(() => {
    // Probe TLS state once. Drives whether the verify-cert step renders.
    api
      .certFingerprint()
      .then((fp) => (tlsActive = fp.tls))
      .catch(() => (tlsActive = false));
  });

  function next() {
    error = null;
    if (step === 'welcome') step = 'register';
    else if (step === 'register') step = tlsActive ? 'verify' : 'share';
    else if (step === 'verify') step = 'share';
    else if (step === 'share') step = 'done';
  }

  function back() {
    error = null;
    if (step === 'register') step = 'welcome';
    else if (step === 'verify') step = 'register';
    else if (step === 'share') step = tlsActive ? 'verify' : 'register';
  }

  async function submitRegister(e: Event) {
    e.preventDefault();
    error = null;
    if (!username.trim() || !password) return;
    if (password !== confirm) {
      error = 'passwords do not match';
      return;
    }
    submitting = true;
    try {
      await register(username.trim(), password);
      // Successful register sets authState to 'ok' as a side effect, but
      // we hold the user inside the wizard so they see the verify + share
      // steps. The TokenGate wrapper still considers them logged in.
      next();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      submitting = false;
    }
  }

  let totalSteps = $derived(tlsActive ? 4 : 3);
  let currentStepIndex = $derived(
    step === 'welcome' ? 1
      : step === 'register' ? 2
      : step === 'verify' ? 3
      : step === 'share' ? (tlsActive ? 4 : 3)
      : totalSteps
  );
</script>

<div class="full-screen">
  <div class="card">
    <header>
      <span class="step-indicator">step {currentStepIndex} of {totalSteps}</span>
      <h2>
        {#if step === 'welcome'}welcome to agentum
        {:else if step === 'register'}create your admin account
        {:else if step === 'verify'}verify the connection
        {:else if step === 'share'}share access with other devices
        {:else}all set
        {/if}
      </h2>
    </header>

    {#if step === 'welcome'}
      <p>
        agentum is a self-hosted control plane for AI coding agents.
        It runs on this machine, manages tmux sessions for each agent,
        and serves a dashboard you can open from your laptop or phone.
      </p>
      <p class="muted">
        This wizard sets up the first admin account, helps you verify
        the connection is genuine, and shows how to share access with
        other devices on your network.
      </p>
      <div class="actions">
        <span class="spacer"></span>
        <button class="primary" type="button" onclick={next}>get started</button>
      </div>

    {:else if step === 'register'}
      <form onsubmit={submitRegister}>
        <p class="muted">
          The first registration becomes the admin account. After this,
          additional users are added from the host with
          <code>agentum auth add &lt;username&gt;</code>.
        </p>

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
            placeholder="min 8 characters"
            autocomplete="new-password"
            required
            minlength={8}
          />
        </label>

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

        {#if error}<p class="err-msg">{error}</p>{/if}

        <div class="actions">
          <button class="ghost" type="button" onclick={back} disabled={submitting}>back</button>
          <button
            class="primary"
            type="submit"
            disabled={!username.trim() || !password || submitting}
          >
            {submitting ? 'creating…' : 'create account →'}
          </button>
        </div>
      </form>

    {:else if step === 'verify'}
      <p class="muted">
        Your browser is talking to agentum over a self-signed TLS
        certificate. Confirm the fingerprint below matches the one
        <code>agentum serve</code> printed on the host's terminal — if
        they're identical, no one on the network is intercepting your
        traffic.
      </p>
      <RemoteAccessInfo compact />
      <div class="actions">
        <button class="ghost" type="button" onclick={back}>back</button>
        <button class="primary" type="button" onclick={next}>looks good →</button>
      </div>

    {:else if step === 'share'}
      <p class="muted">
        Anyone with network access to this host plus your dashboard URL
        and a valid login can use agentum. Share the URL + fingerprint
        with whoever you want to grant access — they'll register through
        the dashboard or you can add them via
        <code>agentum auth add &lt;username&gt;</code> on the host.
      </p>
      <RemoteAccessInfo compact />
      <div class="actions">
        <button class="ghost" type="button" onclick={back}>back</button>
        <button class="primary" type="button" onclick={next}>finish →</button>
      </div>

    {:else}
      <p>
        You're all set. The dashboard is loading the sessions view next.
      </p>
      <p class="muted">
        Need to revisit any of this later? <code>Settings → Remote
        access</code> always shows the current URL and cert fingerprint.
      </p>
      <div class="actions">
        <span class="spacer"></span>
        <a class="primary" href="/sessions">go to dashboard →</a>
      </div>
    {/if}
  </div>
</div>

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
    padding: 1.7rem 1.9rem;
    max-width: 540px;
    width: 100%;
    box-shadow: 0 4px 22px rgba(0, 0, 0, 0.18);
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }
  header {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .step-indicator {
    font-family: var(--font-mono);
    font-size: 0.7rem;
    color: var(--muted);
    letter-spacing: 0.05em;
    text-transform: uppercase;
  }
  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1.18rem;
    color: var(--text);
  }
  p {
    margin: 0;
    color: var(--text-2);
    font-size: 0.9rem;
    line-height: 1.55;
  }
  .muted {
    color: var(--muted);
    font-size: 0.85rem;
    line-height: 1.55;
  }
  code {
    font-family: var(--font-mono);
    color: var(--accent);
    font-size: 0.82rem;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
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
  .actions {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    margin-top: 0.4rem;
  }
  .spacer {
    flex: 1;
  }
  .actions > .ghost,
  .actions > .primary {
    flex: 1;
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
    text-align: center;
    text-decoration: none;
    display: inline-block;
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .ghost {
    background: transparent;
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.6rem 1rem;
    font-family: var(--font-mono);
    font-size: 0.9rem;
    cursor: pointer;
  }
  .ghost:hover {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  .err-msg {
    margin: 0;
    color: var(--danger);
    font-size: 0.85rem;
    font-family: var(--font-mono);
    word-break: break-word;
  }
</style>
