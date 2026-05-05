<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, type Session, type Channel } from '$lib/api';
  import { sessions, loadSessions } from '$stores/sessions';
  import { get } from 'svelte/store';

  interface Node {
    id: string;
    label: string;
    type: 'session';
    status: string;
    x: number;
    y: number;
    vx: number;
    vy: number;
  }

  interface Edge {
    source: string;
    target: string;
    channelId: number;
  }

  let canvas: HTMLCanvasElement;
  let ctx: CanvasRenderingContext2D | null = null;
  // svelte-ignore non_reactive_update
  let nodes: Node[] = [];
  let edges: Edge[] = [];
  let animationId = 0;
  let hoveredId: string | null = null;
  let dragNode: Node | null = null;
  let loading = $state(true);
  let error = $state<string | null>(null);
  let tooltip = $state<{ x: number; y: number; node: Node } | null>(null);

  const NODE_RADIUS = 28;
  const REPULSION = 8000;
  const ATTRACTION = 0.003;
  const DAMPING = 0.85;
  const CENTER_STRENGTH = 0.02;

  onMount(() => {
    loadSessions();
    loadGraphData();
    setupCanvas();
    startSimulation();
    const interval = setInterval(refreshData, 8000);
    return () => {
      clearInterval(interval);
      cancelAnimationFrame(animationId);
    };
  });

  onDestroy(() => cancelAnimationFrame(animationId));

  async function loadGraphData() {
    try {
      const [sess, chans] = await Promise.all([
        api.listSessions(),
        api.listChannels().catch(() => [] as Channel[])
      ]);
      rebuildGraph(sess, chans);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function refreshData() {
    const activeId = dragNode?.id ?? hoveredId;
    try {
      const [sess, chans] = await Promise.all([
        api.listSessions(),
        api.listChannels().catch(() => [] as Channel[])
      ]);
      rebuildGraph(sess, chans);
    } catch { /* silent refresh fail */ }
  }

  function rebuildGraph(sess: Session[], chans: Channel[]) {
    const w = canvas?.width || 600;
    const h = canvas?.height || 400;
    const cx = w / 2;
    const cy = h / 2;

    nodes = sess.map((s, i) => {
      const angle = (2 * Math.PI * i) / sess.length;
      const r = Math.min(w, h) * 0.30;
      return {
        id: s.id,
        label: s.name,
        type: 'session' as const,
        status: s.status,
        x: cx + r * Math.cos(angle),
        y: cy + r * Math.sin(angle),
        vx: 0,
        vy: 0
      };
    });

    edges = [];
    for (const ch of chans) {
      if (nodes.some(n => n.id === ch.a_session) && nodes.some(n => n.id === ch.b_session)) {
        edges.push({ source: ch.a_session, target: ch.b_session, channelId: ch.id });
      }
    }
  }

  function setupCanvas() {
    ctx = canvas.getContext('2d');
    if (!ctx) return;
    resize();
    window.addEventListener('resize', resize);

    canvas.addEventListener('mousemove', (e) => {
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;
      const hit = nodes.find(n => {
        const dx = mx - n.x;
        const dy = my - n.y;
        return dx * dx + dy * dy < NODE_RADIUS * NODE_RADIUS;
      });
      hoveredId = hit?.id ?? null;
      canvas.style.cursor = hit ? 'pointer' : (dragNode ? 'grabbing' : 'default');
      if (hit) {
        tooltip = { x: e.clientX - rect.left, y: e.clientY - rect.top - 12, node: hit };
      } else {
        tooltip = null;
      }
    });

    canvas.addEventListener('mousedown', (e) => {
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;
      const hit = nodes.find(n => {
        const dx = mx - n.x;
        const dy = my - n.y;
        return dx * dx + dy * dy < NODE_RADIUS * NODE_RADIUS;
      });
      if (hit) {
        dragNode = hit;
        canvas.style.cursor = 'grabbing';
      }
    });

    canvas.addEventListener('mouseup', () => {
      if (dragNode) {
        const n = dragNode;
        dragNode = null;
        canvas.style.cursor = 'default';
        handleNodeClick(n);
      }
    });

    canvas.addEventListener('mouseleave', () => {
      dragNode = null;
      hoveredId = null;
      tooltip = null;
      canvas.style.cursor = 'default';
    });
  }

  function resize() {
    if (!canvas) return;
    const parent = canvas.parentElement;
    if (!parent) return;
    const w = parent.clientWidth;
    const h = parent.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    canvas.style.width = w + 'px';
    canvas.style.height = h + 'px';
    if (ctx) ctx.scale(dpr, dpr);
    if (nodes.length === 0) {
      // Re-layout on resize
      const centerX = w / 2;
      const centerY = h / 2;
      const r = Math.min(w, h) * 0.30;
      nodes.forEach((n, i) => {
        const angle = (2 * Math.PI * i) / nodes.length;
        n.x = centerX + r * Math.cos(angle);
        n.y = centerY + r * Math.sin(angle);
      });
    }
  }

  function handleNodeClick(node: Node) {
    if (node.type === 'session') {
      window.location.href = `/sessions/${node.id}`;
    }
  }

  function startSimulation() {
    if (!ctx || !canvas) return;
    const w = () => canvas.width / (window.devicePixelRatio || 1);
    const h = () => canvas.height / (window.devicePixelRatio || 1);

    function step() {
      if (!ctx || !canvas) return;

      // Drag
      if (dragNode) {
        const rect = canvas.getBoundingClientRect();
        dragNode.x = (dragNode as any)._mx ?? dragNode.x;
        dragNode.y = (dragNode as any)._my ?? dragNode.y;
        dragNode.vx = 0;
        dragNode.vy = 0;
      }

      canvas.addEventListener('mousemove', (e) => {
        if (!dragNode) return;
        const rect = canvas.getBoundingClientRect();
        (dragNode as any)._mx = e.clientX - rect.left;
        (dragNode as any)._my = e.clientY - rect.top;
      });

      const Cw = w();
      const Ch = h();

      // Physics
      for (let i = 0; i < nodes.length; i++) {
        const a = nodes[i];
        if (a === dragNode) continue;

        // Repulsion between all pairs
        for (let j = i + 1; j < nodes.length; j++) {
          const b = nodes[j];
          if (b === dragNode) continue;
          let dx = a.x - b.x;
          let dy = a.y - b.y;
          const dist = Math.max(Math.sqrt(dx * dx + dy * dy), 10);
          const force = REPULSION / (dist * dist);
          const fx = (dx / dist) * force;
          const fy = (dy / dist) * force;
          a.vx += fx * 0.016;
          a.vy += fy * 0.016;
          b.vx -= fx * 0.016;
          b.vy -= fy * 0.016;
        }

        // Center gravity
        a.vx += (Cw / 2 - a.x) * CENTER_STRENGTH;
        a.vy += (Ch / 2 - a.y) * CENTER_STRENGTH;
      }

      // Edge attraction
      for (const edge of edges) {
        const a = nodes.find(n => n.id === edge.source);
        const b = nodes.find(n => n.id === edge.target);
        if (!a || !b) continue;
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const dist = Math.max(Math.sqrt(dx * dx + dy * dy), 1);
        const fx = dx * ATTRACTION * dist;
        const fy = dy * ATTRACTION * dist;
        if (a !== dragNode) { a.vx += fx; a.vy += fy; }
        if (b !== dragNode) { b.vx -= fx; b.vy -= fy; }
      }

      // Apply velocity with damping
      for (const n of nodes) {
        if (n === dragNode) continue;
        n.vx *= DAMPING;
        n.vy *= DAMPING;
        n.x += n.vx;
        n.y += n.vy;
        // Boundary
        n.x = Math.max(NODE_RADIUS, Math.min(Cw - NODE_RADIUS, n.x));
        n.y = Math.max(NODE_RADIUS, Math.min(Ch - NODE_RADIUS, n.y));
      }

      render();
      animationId = requestAnimationFrame(step);
    }

    animationId = requestAnimationFrame(step);
  }

  function render() {
    if (!ctx || !canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.width / dpr;
    const h = canvas.height / dpr;

    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    // Background
    const bg = getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() || '#0d0d12';
    ctx.fillStyle = bg;
    ctx.fillRect(0, 0, w, h);

    // Grid pattern (obsidian-style subtle grid)
    ctx.strokeStyle = getComputedStyle(document.documentElement).getPropertyValue('--border').trim() || '#2e2e3a';
    ctx.globalAlpha = 0.15;
    ctx.lineWidth = 0.5;
    const gridSize = 40;
    for (let x = gridSize; x < w; x += gridSize) {
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
      ctx.stroke();
    }
    for (let y = gridSize; y < h; y += gridSize) {
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;

    // Edges
    const edgeColor = getComputedStyle(document.documentElement).getPropertyValue('--border').trim() || '#2e2e3a';
    const accent = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() || '#7f6df2';
    for (const edge of edges) {
      const a = nodes.find(n => n.id === edge.source);
      const b = nodes.find(n => n.id === edge.target);
      if (!a || !b) continue;
      const isRelevant = hoveredId === a.id || hoveredId === b.id;
      ctx.strokeStyle = isRelevant ? accent : edgeColor;
      ctx.globalAlpha = isRelevant ? 0.7 : 0.25;
      ctx.lineWidth = isRelevant ? 1.8 : 1;
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
    ctx.lineWidth = 1;

    // Nodes
    const surface = getComputedStyle(document.documentElement).getPropertyValue('--surface').trim() || '#14141a';
    const success = getComputedStyle(document.documentElement).getPropertyValue('--success').trim() || '#44c9a1';
    const danger = getComputedStyle(document.documentElement).getPropertyValue('--danger').trim() || '#e55360';
    const warn = getComputedStyle(document.documentElement).getPropertyValue('--warn').trim() || '#e2b93b';
    const text = getComputedStyle(document.documentElement).getPropertyValue('--text').trim() || '#d4d4dc';
    const text2 = getComputedStyle(document.documentElement).getPropertyValue('--text-2').trim() || '#8b8ba0';

    for (const node of nodes) {
      const isHovered = hoveredId === node.id;
      const isDragged = dragNode?.id === node.id;
      const scale = isHovered ? 1.15 : 1;

      // Glow on hover
      if (isHovered) {
        ctx.fillStyle = accent;
        ctx.globalAlpha = 0.13;
        ctx.beginPath();
        ctx.arc(node.x, node.y, NODE_RADIUS * scale + 10, 0, Math.PI * 2);
        ctx.fill();
        ctx.globalAlpha = 1;
      }

      // Node circle
      let nodeColor = surface;
      if (node.status === 'running') nodeColor = accent;
      else if (node.status === 'crashed') nodeColor = danger;
      else if (node.status === 'stopped') nodeColor = warn;

      ctx.fillStyle = nodeColor;
      ctx.globalAlpha = isHovered ? 0.95 : 0.85;
      ctx.beginPath();
      ctx.arc(node.x, node.y, NODE_RADIUS * scale, 0, Math.PI * 2);
      ctx.fill();
      ctx.globalAlpha = 1;

      // Border
      ctx.strokeStyle = isHovered ? accent : edgeColor;
      ctx.lineWidth = isHovered ? 2 : 1.2;
      ctx.beginPath();
      ctx.arc(node.x, node.y, NODE_RADIUS * scale, 0, Math.PI * 2);
      ctx.stroke();

      // Status dot
      if (node.status === 'running') {
        ctx.fillStyle = success;
        ctx.beginPath();
        ctx.arc(node.x + NODE_RADIUS * scale * 0.58, node.y - NODE_RADIUS * scale * 0.58, 4.5, 0, Math.PI * 2);
        ctx.fill();
      } else if (node.status === 'crashed') {
        ctx.fillStyle = danger;
        ctx.beginPath();
        ctx.arc(node.x + NODE_RADIUS * scale * 0.58, node.y - NODE_RADIUS * scale * 0.58, 4.5, 0, Math.PI * 2);
        ctx.fill();
      }

      // Label (truncate)
      let label = node.label;
      if (label.length > 14) label = label.slice(0, 12) + '…';
      const fontSize = isHovered ? 11 : 10;
      ctx.font = `${fontSize}px var(--font-mono, monospace)`;
      ctx.fillStyle = text;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      const labelY = node.y + NODE_RADIUS * scale + 16;
      ctx.fillText(label, node.x, labelY);

      // Small status label below
      if (isHovered) {
        ctx.font = '9px var(--font-mono, monospace)';
        ctx.fillStyle = text2;
        ctx.fillText(node.status, node.x, labelY + 14);
      }
    }

    ctx.restore();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'r' || e.key === 'R') {
      e.preventDefault();
      nodes.forEach(n => {
        n.vx = (Math.random() - 0.5) * 10;
        n.vy = (Math.random() - 0.5) * 10;
      });
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<section class="head">
  <div>
    <h2>Graph</h2>
    <p class="muted">Obsidian-style force graph of sessions and channels. Drag nodes to explore. Press <kbd>R</kbd> to scatter.</p>
  </div>
  <div class="legend">
    <span class="leg-item"><span class="leg-dot running"></span> running</span>
    <span class="leg-item"><span class="leg-dot stopped"></span> stopped</span>
    <span class="leg-item"><span class="leg-dot crashed"></span> crashed</span>
    <span class="leg-item"><span class="leg-line"></span> channel</span>
  </div>
</section>

{#if error}
  <div class="error">{error}</div>
{/if}

<div class="canvas-wrap">
  {#if loading}
    <div class="loading">building graph…</div>
  {/if}
  <canvas bind:this={canvas}></canvas>
  {#if nodes.length === 0 && !loading}
    <div class="empty">
      <div class="empty-title">No sessions to graph</div>
      <div class="empty-body">Create sessions and channels to see them visualized here.</div>
    </div>
  {/if}
  {#if tooltip}
    <div class="tooltip" style="left:{tooltip.x}px;top:{tooltip.y}px">
      <strong>{tooltip.node.label}</strong>
      <span class="tooltip-status">{tooltip.node.status}</span>
    </div>
  {/if}
</div>

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 0.75rem;
    flex-wrap: wrap;
  }
  h2 {
    font-family: var(--font-display);
    font-weight: 600;
    margin: 0 0 0.2rem;
    font-size: 1.4rem;
  }
  .muted { color: var(--muted); margin: 0; font-size: 0.85rem; }
  .muted kbd {
    font-family: var(--font-mono);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0.05em 0.4em;
    font-size: 0.8rem;
  }
  .legend {
    display: flex;
    gap: 1rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .leg-item {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.75rem;
    color: var(--text-2);
    font-family: var(--font-mono);
  }
  .leg-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .leg-dot.running { background: var(--accent); }
  .leg-dot.stopped { background: var(--warn); }
  .leg-dot.crashed { background: var(--danger); }
  .leg-line {
    width: 16px;
    height: 1px;
    background: var(--border);
  }
  .canvas-wrap {
    position: relative;
    flex: 1;
    min-height: 400px;
    max-height: calc(100vh - 200px);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg);
    overflow: hidden;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }
  .loading {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: 0.85rem;
  }
  .empty {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    text-align: center;
  }
  .empty-title {
    font-family: var(--font-display);
    font-size: 1.1rem;
    color: var(--text-2);
    margin-bottom: 0.3rem;
  }
  .empty-body {
    color: var(--muted);
    font-size: 0.85rem;
  }
  .tooltip {
    position: absolute;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.4rem 0.7rem;
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--text);
    pointer-events: none;
    transform: translate(-50%, -100%);
    box-shadow: 0 4px 14px rgba(0,0,0,0.25);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.15rem;
    white-space: nowrap;
  }
  .tooltip-status {
    color: var(--muted);
    font-size: 0.68rem;
  }
  .error {
    padding: 0.7rem 1rem;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-family: var(--font-mono);
    font-size: 0.85rem;
    margin-bottom: 0.6rem;
  }
</style>
