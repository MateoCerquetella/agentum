<script lang="ts">
  /**
   * Bigger, denser hero stat than `StatTile`. Carries:
   *   - a label
   *   - a primary value (large)
   *   - up to 3 sub-rows (tag chips with their own color)
   *   - optional accent stripe on the left edge
   *
   * Designed to replace 2 small tiles with a single richer card so the
   * hero stops reading like a vanity dashboard.
   */
  import type { Snippet } from 'svelte';

  interface Tag { label: string; color?: string; }

  interface Props {
    k: string;
    v: string;
    accent?: string;
    tags?: Tag[];
    foot?: string;
    children?: Snippet;
  }
  let { k, v, accent, tags = [], foot, children }: Props = $props();
</script>

<div class="card" style:border-left-color={accent ?? 'transparent'}>
  <div class="head">
    <span class="k">{k}</span>
  </div>
  <div class="v" style:color={accent ?? 'var(--fg)'}>{v}</div>
  {#if tags.length > 0}
    <div class="tags">
      {#each tags as t (t.label)}
        <span class="tag" style:color={t.color ?? 'var(--fg-2)'} style:border-color={t.color ? `color-mix(in oklab, ${t.color} 35%, var(--border-2))` : 'var(--border-2)'}>
          {#if t.color}<span class="tag-dot" style:background={t.color}></span>{/if}
          {t.label}
        </span>
      {/each}
    </div>
  {/if}
  {#if children}
    <div class="extra">{@render children()}</div>
  {/if}
  {#if foot}
    <div class="foot">{foot}</div>
  {/if}
</div>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-left-width: 3px;
    border-radius: var(--radius-lg);
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .k {
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .v {
    font-family: var(--display);
    font-size: 28px;
    line-height: 1;
    letter-spacing: -0.02em;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 7px;
    border: 1px solid var(--border-2);
    border-radius: 999px;
    background: var(--bg-2);
    font-family: var(--mono);
    font-size: 10.5px;
  }
  .tag-dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
  }
  .extra { margin-top: 2px; }
  .foot {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    margin-top: auto;
  }
</style>
