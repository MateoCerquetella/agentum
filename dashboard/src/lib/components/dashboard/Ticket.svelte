<script lang="ts">
  import type { BoardItem } from '$lib/api';

  /**
   * Kanban card. The whole card is the drag handle. Tool dot color
   * comes from the optional `tool` discriminator (claude/codex/gemini/
   * hermes); label pill from `lbl`.
   */
  interface Props {
    tk: BoardItem;
    dragging?: boolean;
    onDragStart?: (e: DragEvent) => void;
    onDragEnd?: () => void;
    onClick?: () => void;
  }
  let { tk, dragging = false, onDragStart, onDragEnd, onClick }: Props = $props();

  const lblText = $derived(tk.lbl ?? 'task');
  const toolClass = $derived(tk.tool ?? '');
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
  <div class="tk-foot">
    <span class="dot"></span>
    <span>{tk.claimed_by ?? 'unclaimed'}</span>
    <span class="lbls">
      <span class="lbl {tk.lbl ?? ''}">{lblText}</span>
    </span>
  </div>
</div>
