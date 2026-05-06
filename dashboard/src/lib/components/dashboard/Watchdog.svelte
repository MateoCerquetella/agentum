<script lang="ts">
  import type { WatchdogEvent } from '$lib/api';

  /** Vertical event feed — newest first. Auto-scrolls; pauses on hover. */
  interface Props {
    feed: WatchdogEvent[];
    limit?: number;
  }
  let { feed, limit }: Props = $props();

  const visible = $derived(typeof limit === 'number' ? feed.slice(0, limit) : feed);

  function ts(iso: string): string {
    if (!iso) return '';
    // Accept already-formatted HH:MM:SS or ISO string.
    if (/^\d{2}:\d{2}/.test(iso)) return iso;
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleTimeString('en-GB', { hour12: false });
  }
</script>

<div class="wd">
  {#each visible as ev, i (ev.ts + ':' + i)}
    <div class={`row ${ev.kind}`}>
      <span class="ts">{ts(ev.ts)}</span>
      <div class="ev">
        <span class="et">{ev.label}</span>
        <div class="msg">{ev.msg}</div>
      </div>
    </div>
  {/each}
  {#if visible.length === 0}
    <div class="empty">No events yet.</div>
  {/if}
</div>

<style>
  /* .wd / .wd .row styles inherit from _design.css. */
  .empty {
    padding: 16px 0;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--fg-3);
    text-align: center;
  }
</style>
