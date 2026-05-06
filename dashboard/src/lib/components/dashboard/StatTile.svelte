<script lang="ts">
  /**
   * Small reusable stat — "Live · 5 / 7 panes". Colored value when
   * the stat carries a tone; otherwise reads white on surface.
   */
  interface Props {
    k: string;
    v: string;
    sub?: string;
    accent?: string;
  }
  let { k, v, sub, accent }: Props = $props();
</script>

<div class="tile">
  <div class="micro k">{k}</div>
  <div class="v" style:color={accent ?? 'var(--fg)'}>{v}</div>
  {#if sub}<div class="sub">{sub}</div>{/if}
</div>

<style>
  .tile {
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-lg);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .v {
    font-size: 26px;
    letter-spacing: -0.02em;
    line-height: 1;
    font-family: var(--display);
  }
  .sub {
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
</style>
