<script lang="ts">
  import Icon from '$components/Icon.svelte';
</script>

<section class="head">
  <h2>Settings</h2>
  <p class="muted">Coming soon. Settings will include session defaults, notification preferences, and advanced configuration.</p>
</section>

<div class="empty">
  <div class="empty-icon">
    <Icon name="settings" size={48} />
  </div>
  <div class="empty-title">Settings are not yet available</div>
  <div class="empty-body">
    Use the CLI for configuration: <code>agentum config get | set | edit</code>
  </div>
</div>

<style>
  .head {
    margin-bottom: 1rem;
  }
  h2 {
    font-family: var(--font-display);
    font-weight: 600;
    margin: 0 0 0.25rem;
    font-size: 1.4rem;
  }
  .muted { color: var(--muted); margin: 0; font-size: 0.85rem; }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: 4rem 2rem;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    gap: 0.6rem;
  }
  .empty-icon { color: var(--muted); opacity: 0.4; }
  .empty-title {
    font-family: var(--font-display);
    font-size: 1.05rem;
    color: var(--text-2);
  }
  .empty-body { color: var(--muted); font-size: 0.85rem; }
</style>
