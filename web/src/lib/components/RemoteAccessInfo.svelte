<script lang="ts">
  /**
   * Reusable "how others connect to this server" panel. Used by the
   * onboarding wizard's verify-cert step and the Settings → Remote Access
   * page (so the operator can come back and grab the URL + fingerprint
   * any time).
   *
   * Shows:
   *   - the URL a second device should visit (this page's origin)
   *   - the SHA-256 fingerprint of the active TLS cert (or a notice
   *     when running with --no-tls)
   *   - a 3-line cheat sheet for the access modes (LAN / public / VPN)
   *
   * No state of its own — fetches `/api/cert/fingerprint` once on mount.
   */
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { CertFingerprint } from '$lib/api';

  interface Props {
    /// When true, render in a "card embedded in the wizard" style; when
    /// false, the standalone Settings page style.
    compact?: boolean;
  }
  let { compact = false }: Props = $props();

  let fp = $state<CertFingerprint | null>(null);
  let copied = $state<'url' | 'fp' | null>(null);
  let url = $derived(typeof location !== 'undefined' ? location.origin : '');

  onMount(async () => {
    try {
      fp = await api.certFingerprint();
    } catch {
      fp = { sha256: '', tls: false };
    }
  });

  async function copy(text: string, label: 'url' | 'fp') {
    try {
      await navigator.clipboard.writeText(text);
      copied = label;
      setTimeout(() => (copied = null), 1500);
    } catch {
      /* clipboard API not available — user copies manually */
    }
  }
</script>

<div class="info" class:compact>
  <section class="row">
    <div class="row-head">
      <h3>Dashboard URL</h3>
      <button class="copy" type="button" onclick={() => copy(url, 'url')}>
        {copied === 'url' ? 'copied' : 'copy'}
      </button>
    </div>
    <code class="value">{url || '…'}</code>
    <p class="hint">
      Paste this into a browser on any device that can reach this host
      over the network.
    </p>
  </section>

  <section class="row">
    <div class="row-head">
      <h3>TLS cert fingerprint (SHA-256)</h3>
      {#if fp?.sha256}
        <button class="copy" type="button" onclick={() => copy(fp!.sha256, 'fp')}>
          {copied === 'fp' ? 'copied' : 'copy'}
        </button>
      {/if}
    </div>
    {#if fp == null}
      <code class="value muted">loading…</code>
    {:else if !fp.tls}
      <code class="value muted">running without TLS — no cert to verify</code>
      <p class="hint">
        Start with <code>agentum serve</code> (without <code>--no-tls</code>)
        to get a self-signed cert and a fingerprint to pin.
      </p>
    {:else}
      <code class="value">{fp.sha256}</code>
      <p class="hint">
        Verify this matches the line <code>agentum serve</code> printed
        on the host's terminal — that's how you confirm nobody on the
        network is intercepting the connection.
      </p>
    {/if}
  </section>

  <section class="row modes">
    <h3>How another device connects</h3>
    <ul>
      <li>
        <strong>Same LAN.</strong> If the other device is on your home or
        office network, paste the URL above. Browsers will show a
        "self-signed" warning — accept it after verifying the fingerprint.
      </li>
      <li>
        <strong>Public IP / port forward.</strong> Forward port 8822 from
        your router to this host, then share
        <code>https://&lt;your-public-ip&gt;:8822</code>. Same fingerprint
        check applies.
      </li>
      <li>
        <strong>VPN (WireGuard / Tailscale).</strong> If both devices are
        on the same VPN, treat it like LAN — use this host's VPN address
        in the URL. No port forwarding needed.
      </li>
    </ul>
    <p class="hint">
      Friends on iPhone / Android: have them paste the URL into Safari /
      Chrome, accept the cert warning, then verify the fingerprint shown
      in the address bar's lock icon matches the one above.
    </p>
  </section>
</div>

<style>
  .info {
    display: flex;
    flex-direction: column;
    gap: 1.4rem;
  }
  .info.compact {
    gap: 1rem;
  }
  .row {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .row-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
  }
  h3 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 0.92rem;
    color: var(--text);
  }
  .value {
    display: block;
    padding: 0.55rem 0.75rem;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.82rem;
    color: var(--text);
    word-break: break-all;
    line-height: 1.4;
  }
  .value.muted {
    color: var(--muted);
  }
  .hint {
    margin: 0;
    font-size: 0.78rem;
    color: var(--muted);
    line-height: 1.5;
  }
  .copy {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--muted);
    border-radius: 4px;
    padding: 0.15rem 0.55rem;
    font-family: var(--font-mono);
    font-size: 0.72rem;
    cursor: pointer;
  }
  .copy:hover {
    color: var(--text);
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  ul {
    margin: 0;
    padding-left: 1.05rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    font-size: 0.85rem;
    color: var(--text-2);
    line-height: 1.55;
  }
  ul code {
    font-family: var(--font-mono);
    color: var(--accent);
    font-size: 0.78rem;
  }
  strong {
    color: var(--text);
    font-weight: 600;
  }
</style>
