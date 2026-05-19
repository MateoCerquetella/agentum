<script lang="ts">
  import type { BoardItem } from '$lib/api';

  /**
   * Dense kanban card. Optimised for stacked swimlanes where the
   * lane header already names the project — so the card drops the
   * workdir trail and keeps everything to a tight two-line layout:
   *
   *   • key + tool dot + claim pill + (server) + (session ↗) + lbl
   *   • title (clamped to 2 lines)
   *
   * Whole card is the drag handle; the session arrow stops propagation
   * so click→jump doesn't also fire the parent's edit-dialog handler.
   */
  interface Props {
    tk: BoardItem;
    dragging?: boolean;
    /** Optional `@profile` chip rendered when the parent passes a
     *  source label. Only set on multi-profile setups. */
    sourceLabel?: string | null;
    /** Comment count from a parent-level lookup so the card can render
     *  a 💬N chip without each card refetching. 0 hides the chip. */
    commentCount?: number;
    /** True when the parent's drag is hovering this card and the drop
     *  would land *above* it. The component renders a thin insertion
     *  line at the top edge. */
    dropAbove?: boolean;
    onDragStart?: (e: DragEvent) => void;
    onDragEnd?: () => void;
    onDragOver?: (e: DragEvent) => void;
    onDragLeave?: (e: DragEvent) => void;
    onDrop?: (e: DragEvent) => void;
    onClick?: () => void;
  }
  let {
    tk,
    dragging = false,
    sourceLabel = null,
    commentCount = 0,
    dropAbove = false,
    onDragStart,
    onDragEnd,
    onDragOver,
    onDragLeave,
    onDrop,
    onClick
  }: Props = $props();

  const lblText = $derived(tk.lbl ?? 'task');
  const toolClass = $derived(tk.tool ?? '');
  const claimShort = $derived(
    tk.claimed_by ? tk.claimed_by.replace(/^web-/, '').slice(0, 6) : null
  );
</script>

<div
  class="ticket {toolClass} {lblText}"
  class:dragging
  class:unclaimed={tk.claimed_by == null}
  class:drop-above={dropAbove}
  draggable="true"
  ondragstart={onDragStart}
  ondragend={onDragEnd}
  ondragover={onDragOver}
  ondragleave={onDragLeave}
  ondrop={onDrop}
  onclick={onClick}
  onkeydown={(e) => { if (onClick && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onClick(); } }}
  role="button"
  tabindex="0"
>
  <div class="tk-head">
    <span class="dot" aria-hidden="true"></span>
    <span class="tk-k">{tk.key}</span>
    {#if claimShort}
      <span class="claim-pill" title={tk.claimed_by ?? ''}>{claimShort}</span>
    {/if}
    <span class="tk-spacer"></span>
    {#if sourceLabel}
      <span class="src" title={sourceLabel}>@{sourceLabel}</span>
    {/if}
    {#if commentCount > 0}
      <span class="cmt" title={`${commentCount} comment${commentCount === 1 ? '' : 's'}`}>
        💬 {commentCount}
      </span>
    {/if}
    {#if tk.session_id}
      <!-- stopPropagation so the arrow jumps to the session page
           without ALSO opening the edit dialog underneath. -->
      <a
        class="jump"
        href={`/sessions/${tk.session_id}`}
        title={`Open session ${tk.session_id.slice(0, 8)}…`}
        onclick={(e) => e.stopPropagation()}
      >↗</a>
    {/if}
    <span class="lbl {tk.lbl ?? ''}">{lblText}</span>
  </div>
  <div class="tk-t" title={tk.title}>{tk.title}</div>
</div>

<style>
  /* Cards are tighter — relies on global .ticket base from _design.css
     for the box/border, then overrides typography + spacing here. */
  :global(.lane .ticket) {
    padding: 8px 10px;
    gap: 4px;
    transition: border-color var(--t-hover), transform var(--t-hover), box-shadow var(--t-hover);
    position: relative;
  }
  :global(.lane .ticket:hover) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
  }
  :global(.lane .ticket.unclaimed) {
    border-style: dashed;
  }
  /* Insertion line — drawn at the top edge when a drag is hovering
     above this card and intends to drop above it. */
  :global(.lane .ticket.drop-above::before) {
    content: '';
    position: absolute;
    left: 4px;
    right: 4px;
    top: -3px;
    height: 2px;
    background: var(--cta);
    border-radius: 2px;
    pointer-events: none;
  }

  .tk-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--fg-3);
    line-height: 1;
  }
  .tk-head .dot {
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill);
    flex-shrink: 0;
    background: var(--fg-3);
  }
  :global(.ticket.claude) .tk-head .dot { background: var(--tool-claude); }
  :global(.ticket.codex)  .tk-head .dot { background: var(--tool-codex); }
  :global(.ticket.cursor) .tk-head .dot { background: var(--tool-cursor, var(--cta)); }
  :global(.ticket.gemini) .tk-head .dot { background: var(--tool-gemini); }
  :global(.ticket.hermes) .tk-head .dot { background: var(--tool-hermes); }

  .tk-k { color: var(--fg-2); letter-spacing: 0.02em; }
  .tk-spacer { flex: 1; }

  .claim-pill {
    padding: 1px 6px;
    border-radius: var(--radius-pill);
    background: color-mix(in srgb, var(--cta) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--cta) 40%, var(--border-2));
    color: var(--fg);
    font-size: 9.5px;
    line-height: 1.2;
  }

  .src {
    font-size: 9.5px;
    color: var(--fg-3);
  }
  /* Comment count chip — only renders when commentCount > 0. */
  .cmt {
    font-size: 9.5px;
    color: var(--fg-3);
    padding: 0 4px;
    border-left: 1px solid var(--border-2);
    margin-left: 2px;
  }
  .jump {
    color: var(--cta);
    font-size: 11.5px;
    text-decoration: none;
    line-height: 1;
  }
  .jump:hover { color: var(--fg); }

  .lbl {
    padding: 1px 6px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border-2);
    background: var(--surface);
    color: var(--fg-3);
    font-size: 9.5px;
    line-height: 1.2;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .lbl.bug   { color: var(--crash); border-color: rgba(221,0,0,0.4); }
  .lbl.feat  { color: var(--green); border-color: rgba(25,214,0,0.4); }
  .lbl.chore { color: var(--blu);   border-color: rgba(85,190,255,0.4); }
  .lbl.spike { color: var(--amber); border-color: rgba(255,180,84,0.4); }

  /* Title is line-clamped so long titles don't break lane height. */
  .tk-t {
    color: var(--fg);
    font-size: 12.5px;
    line-height: 1.35;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    word-break: break-word;
  }
</style>
