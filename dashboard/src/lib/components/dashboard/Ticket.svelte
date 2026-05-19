<script lang="ts">
  import type { BoardItem } from '$lib/api';

  /**
   * Kanban card. The whole card is the drag handle. Tool dot color
   * comes from the optional `tool` discriminator (claude/codex/gemini/
   * hermes); label pill from `lbl`. When `workdir` is set, surface a
   * short trail so the board reads as folder + agent + lbl per card.
   */
  interface Props {
    tk: BoardItem;
    dragging?: boolean;
    /** When set, render a small `@profile` chip in the foot to surface the
     *  ticket's source server. Hidden on single-profile setups. */
    sourceLabel?: string | null;
    onDragStart?: (e: DragEvent) => void;
    onDragEnd?: () => void;
    onClick?: () => void;
  }
  let { tk, dragging = false, sourceLabel = null, onDragStart, onDragEnd, onClick }: Props = $props();

  const lblText = $derived(tk.lbl ?? 'task');
  const toolClass = $derived(tk.tool ?? '');

  /// Compress a workdir path so the card stays one-line. Keep at most
  /// the final two path segments and a leading `~` or `/` so the trail
  /// reads like "~/foo/bar" instead of the full absolute path.
  function compressPath(p: string | null | undefined): string {
    if (!p) return '';
    const home = '/home/'; // good-enough heuristic; collapses /home/<user>/ to ~/
    let s = p;
    const homeIdx = s.indexOf(home);
    if (homeIdx === 0) {
      const tail = s.slice(home.length);
      const slash = tail.indexOf('/');
      s = slash < 0 ? '~' : `~/${tail.slice(slash + 1)}`;
    }
    const segs = s.split('/').filter(Boolean);
    if (segs.length <= 2) return s;
    const prefix = s.startsWith('~') ? '~/' : '/';
    return `${prefix}…/${segs.slice(-2).join('/')}`;
  }

  const trail = $derived(compressPath(tk.workdir));
</script>

<div
  class="ticket {toolClass} {lblText}"
  class:dragging
  draggable="true"
  ondragstart={onDragStart}
  ondragend={onDragEnd}
  onclick={onClick}
  onkeydown={(e) => { if (onClick && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onClick(); } }}
  role="button"
  tabindex="0"
>
  <div class="tk-k">{tk.key}</div>
  <div class="tk-t">{tk.title}</div>
  {#if trail}
    <div class="tk-trail" title={tk.workdir ?? ''}>{trail}</div>
  {/if}
  <div class="tk-foot">
    <span class="dot"></span>
    <span>{tk.claimed_by ?? 'unclaimed'}</span>
    {#if sourceLabel}
      <span class="src">@{sourceLabel}</span>
    {/if}
    {#if tk.session_id}
      <!-- stopPropagation so the arrow lands on the session page
           without ALSO opening the edit dialog underneath. -->
      <a
        class="jump"
        href={`/sessions/${tk.session_id}`}
        title={`Open session ${tk.session_id.slice(0, 8)}…`}
        onclick={(e) => e.stopPropagation()}
      >↗</a>
    {/if}
    <span class="lbls">
      <span class="lbl {tk.lbl ?? ''}">{lblText}</span>
    </span>
  </div>
</div>

<style>
  .tk-trail {
    margin: 4px 0 2px;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--fg-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Source-server chip in the foot — only rendered on multi-profile
     setups. Inherits the foot's existing mono font; muted by default. */
  .src {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    padding: 0 4px;
    border-left: 1px solid var(--border-2);
    margin-left: 2px;
  }
  /* Session jump-arrow. Renders only when tk.session_id is set; click
     escapes the card-level dialog handler via stopPropagation. */
  .jump {
    color: var(--cta);
    font-size: 12px;
    text-decoration: none;
    padding: 0 4px;
    line-height: 1;
  }
  .jump:hover { color: var(--fg); }
</style>
