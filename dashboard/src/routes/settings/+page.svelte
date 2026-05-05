<script lang="ts">
  import Icon from '$components/Icon.svelte';
  import RemoteAccessInfo from '$components/RemoteAccessInfo.svelte';
</script>

<section class="head">
  <h2>Settings</h2>
  <p class="muted">
    Most configuration still lives in <code>agentum config get | set | edit</code>
    on the host. Here for now: how to connect other devices to this server.
  </p>
</section>

<section class="block">
  <div class="block-head">
    <Icon name="settings" size={20} />
    <h3>Remote access</h3>
  </div>
  <p class="muted block-body">
    Share the URL and verify the fingerprint with whoever you want to
    grant access. Anyone who can reach this host on the network plus a
    valid login can use agentum.
  </p>
  <RemoteAccessInfo />
</section>

<section class="block dim">
  <div class="block-head">
    <Icon name="settings" size={20} />
    <h3>More settings</h3>
  </div>
  <p class="muted block-body">
    Session defaults, notification preferences, and advanced configuration
    are still being moved out of the CLI. For now use:
    <code>agentum config get | set | edit</code>.
  </p>
</section>

<style>
  .head {
    margin-bottom: 1.4rem;
  }
  h2 {
    font-family: var(--font-display);
    font-weight: 600;
    margin: 0 0 0.35rem;
    font-size: 1.4rem;
  }
  .muted { color: var(--muted); margin: 0; font-size: 0.85rem; line-height: 1.5; }
  .block {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1.25rem 1.4rem;
    margin-bottom: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .block.dim {
    opacity: 0.7;
  }
  .block-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--muted);
  }
  .block-head h3 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1rem;
    color: var(--text);
  }
  .block-body {
    margin: 0;
  }
  code {
    font-family: var(--font-mono);
    color: var(--accent);
    font-size: 0.82rem;
  }
</style>
