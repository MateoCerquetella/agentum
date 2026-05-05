<script lang="ts">
  import Terminal from './Terminal.svelte';
  import StatusPill from './StatusPill.svelte';
  import { patchLayout, bringToFront, type PanelLayout } from '$stores/canvas';
  import type { Session } from '$lib/api';

  interface Props {
    session: Session;
    layout: PanelLayout;
    /** When true, terminal won't forward keystrokes — useful for snapshot mode. */
    readonly?: boolean;
    onMaximize?: (id: string) => void;
    onOpen?: (id: string) => void;
    /** Whether this panel is currently maximized in the canvas. */
    maximized?: boolean;
  }
  let { session, layout, readonly = false, onMaximize, onOpen, maximized = false }: Props = $props();

  const MIN_W = 280;
  const MIN_H = 180;

  // Reactive flags so `class:active` updates while the user drags/resizes.
  let dragging = $state(false);
  let resizing = $state<null | 'r' | 'b' | 'br'>(null);
  let startX = 0;
  let startY = 0;
  let startLayout: PanelLayout = { x: 0, y: 0, w: 0, h: 0, z: 0 };
  let pointerId: number | null = null;

  const YOLO_FLAG = '--dangerously-skip-permissions';
  const isYolo = $derived(session.flags.includes(YOLO_FLAG));

  function captureStart(e: PointerEvent, mode: 'drag' | 'r' | 'b' | 'br') {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    bringToFront(session.id);
    if (mode === 'drag') dragging = true;
    else resizing = mode;
    startX = e.clientX;
    startY = e.clientY;
    startLayout = { ...layout };
    pointerId = e.pointerId;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging && !resizing) return;
    const dx = e.clientX - startX;
    const dy = e.clientY - startY;
    if (dragging) {
      patchLayout(session.id, {
        x: Math.max(0, startLayout.x + dx),
        y: Math.max(0, startLayout.y + dy)
      });
    } else if (resizing === 'r') {
      patchLayout(session.id, { w: Math.max(MIN_W, startLayout.w + dx) });
    } else if (resizing === 'b') {
      patchLayout(session.id, { h: Math.max(MIN_H, startLayout.h + dy) });
    } else if (resizing === 'br') {
      patchLayout(session.id, {
        w: Math.max(MIN_W, startLayout.w + dx),
        h: Math.max(MIN_H, startLayout.h + dy)
      });
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (pointerId !== null) {
      try { (e.currentTarget as HTMLElement).releasePointerCapture(pointerId); } catch { /* ignore */ }
    }
    dragging = false;
    resizing = null;
    pointerId = null;
  }

  function onHeaderDoubleClick() {
    onMaximize?.(session.id);
  }

  function focusPanel() { bringToFront(session.id); }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="panel"
  class:maximized
  class:active={dragging || resizing !== null}
  style:--x="{layout.x}px"
  style:--y="{layout.y}px"
  style:--w="{layout.w}px"
  style:--h="{layout.h}px"
  style:--z={layout.z}
  onmousedown={focusPanel}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <header
    class="head"
    onpointerdown={(e) => captureStart(e, 'drag')}
    ondblclick={onHeaderDoubleClick}
  >
    <span class="grip" aria-hidden="true">⋮⋮</span>
    <span class="name" title={session.workdir}>{session.name}</span>
    <span class="tool mono">{session.tool}</span>
    {#if isYolo}
      <span class="yolo-dot" title="YOLO mode — permissions auto-approved">⚡</span>
    {/if}
    <span class="spacer"></span>
    <StatusPill status={session.status} />
    <button
      class="iconbtn"
      type="button"
      onclick={() => onOpen?.(session.id)}
      title="Open session page"
    >
      ↗
    </button>
    <button
      class="iconbtn"
      type="button"
      onclick={() => onMaximize?.(session.id)}
      title={maximized ? 'Restore' : 'Maximize'}
    >
      {maximized ? '▢' : '▣'}
    </button>
  </header>

  <div class="body">
    <Terminal sessionId={session.id} {readonly} compact />
  </div>

  {#if !maximized}
    <!-- Resize handles. Pointer events enabled only on these strips. -->
    <div class="resize r"  onpointerdown={(e) => captureStart(e, 'r')}></div>
    <div class="resize b"  onpointerdown={(e) => captureStart(e, 'b')}></div>
    <div class="resize br" onpointerdown={(e) => captureStart(e, 'br')}></div>
  {/if}
</div>

<style>
  .panel {
    position: absolute;
    left: var(--x);
    top: var(--y);
    width: var(--w);
    height: var(--h);
    z-index: var(--z);
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.18), 0 1px 0 rgba(255, 255, 255, 0.02) inset;
    transition: box-shadow 150ms ease, border-color 150ms ease;
  }
  .panel.active {
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.28), 0 0 0 1px color-mix(in srgb, var(--accent) 35%, transparent);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
  }
  .panel.maximized {
    left: 0 !important;
    top: 0 !important;
    width: 100% !important;
    height: 100% !important;
    border-radius: 0;
    border: 0;
    z-index: 9999;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.65rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface-2);
    cursor: grab;
    user-select: none;
  }
  .head:active { cursor: grabbing; }
  .grip {
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.7rem;
    letter-spacing: -0.1em;
  }
  .name {
    font-family: var(--font-display);
    font-weight: 500;
    font-size: 0.85rem;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 40%;
  }
  .tool {
    font-size: 0.7rem;
    color: var(--accent);
  }
  .mono { font-family: var(--font-mono); }
  .spacer { flex: 1; }
  .iconbtn {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 5px;
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.78rem;
    padding: 0.15rem 0.4rem;
    cursor: pointer;
    transition: color 120ms ease, border-color 120ms ease, background 120ms ease;
  }
  .iconbtn:hover {
    color: var(--text);
    border-color: var(--border);
    background: var(--surface);
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .body :global(.wrap) {
    border: 0;
    border-radius: 0;
    background: var(--bg);
  }

  .resize { position: absolute; background: transparent; }
  .resize.r  { top: 36px; right: 0; bottom: 8px; width: 6px; cursor: ew-resize; }
  .resize.b  { left: 0; right: 8px; bottom: 0; height: 6px; cursor: ns-resize; }
  .resize.br { right: 0; bottom: 0; width: 14px; height: 14px; cursor: nwse-resize; }
  .resize.br::after {
    content: '';
    position: absolute;
    right: 3px;
    bottom: 3px;
    width: 8px;
    height: 8px;
    border-right: 2px solid var(--border);
    border-bottom: 2px solid var(--border);
    pointer-events: none;
  }
  .panel:hover .resize.br::after { border-color: var(--muted); }
</style>
