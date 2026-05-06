<script lang="ts">
  /**
   * Unified-diff render with add/del row coloring. The shape is small:
   * a header path + adds/deletes counters, then a body of context /
   * additions / deletions with line numbers.
   *
   * The session-detail page passes hunks once the server exposes a
   * diff endpoint; until then it renders an empty stub.
   */
  import type { DiffLine } from './types';

  interface Props {
    path: string;
    added: number;
    deleted: number;
    lines: DiffLine[];
  }
  let { path, added, deleted, lines }: Props = $props();
</script>

<div class="diff">
  <div class="dh">
    <span style="color: var(--fg);">{path}</span>
    <span style="color: var(--fg-3);">·</span>
    <span class="badge add">+{added}</span>
    <span class="badge del">-{deleted}</span>
  </div>
  <div class="body">
    {#each lines as l, i (i)}
      {#if l.kind === 'blank'}
        <div class="ln blank"><span class="num">&nbsp;</span><span>&nbsp;</span></div>
      {:else}
        <div class="ln {l.kind === 'ctx' ? '' : l.kind}">
          <span class="num">{l.num}</span>
          <span>{l.text}</span>
        </div>
      {/if}
    {/each}
  </div>
</div>
