<script lang="ts">
  import { connStatus } from '$stores/events';

  // Show the strip only once the events-bus WS has failed to reconnect
  // at least twice. A 1-attempt blip means the bus closed and the very
  // next setTimeout fired connect() — usually sub-second and not worth
  // flashing a banner over. Two failures in a row signals a real outage.
  const SHOWAT_ATTEMPT = 2;

  const visible = $derived(
    $connStatus.state === 'reconnecting' && $connStatus.attempt >= SHOWAT_ATTEMPT,
  );

  // Format the retry countdown rather than freezing on the WS handler's
  // setTimeout. The store update fires once at backoff start; we lock
  // a target wall-clock and tick toward it locally.
  let now = $state(Date.now());
  let targetAt = $state<number | null>(null);

  $effect(() => {
    const s = $connStatus;
    if (s.state === 'reconnecting') {
      targetAt = Date.now() + s.nextDelayMs;
    } else {
      targetAt = null;
    }
  });

  $effect(() => {
    if (!visible) return;
    const handle = setInterval(() => {
      now = Date.now();
    }, 200);
    return () => clearInterval(handle);
  });

  const secsLeft = $derived(
    targetAt ? Math.max(0, (targetAt - now) / 1000) : 0,
  );

  const label = $derived.by(() => {
    if ($connStatus.state !== 'reconnecting') return '';
    const a = $connStatus.attempt;
    if ($connStatus.nextDelayMs === 0) {
      // HTTP-failure-driven state — no backoff, just an outage signal.
      return `Daemon not responding · ${a} failed request${a === 1 ? '' : 's'}`;
    }
    return `Reconnecting to the agentum daemon · attempt ${a} · retry in ${secsLeft.toFixed(1)}s`;
  });
</script>

{#if visible}
  <div class="banner" role="status" aria-live="polite">
    <span class="spin" aria-hidden="true">⟳</span>
    <span class="text">{label}</span>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 1rem;
    background: var(--warn-bg, #3a2a08);
    color: var(--warn-fg, #f0c674);
    border-bottom: 1px solid var(--warn-border, #5a4112);
    font-size: 0.85rem;
    font-weight: 500;
    /* Sits just under the topbar; sticky so it stays visible while the
       user scrolls a long session view. */
    position: sticky;
    top: 0;
    z-index: 50;
  }

  .spin {
    display: inline-block;
    animation: spin 1.4s linear infinite;
    font-size: 0.95rem;
  }

  .text {
    line-height: 1.2;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to   { transform: rotate(360deg); }
  }

  @media (prefers-reduced-motion: reduce) {
    .spin { animation: none; }
  }
</style>
