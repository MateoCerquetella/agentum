<script lang="ts">
  /**
   * Tiny inline area chart. Pure SVG, no deps. Auto-scales to data;
   * draws nothing on empty input.
   */
  interface Props {
    data: number[];
    color?: string;
    height?: number;
  }
  let { data, color = 'var(--green)', height = 28 }: Props = $props();

  const view = $derived.by(() => {
    if (!data.length) return null;
    const w = 100;
    const h = height;
    const max = Math.max(...data, 1);
    const pts = data.map((v, i) => `${(i / Math.max(data.length - 1, 1)) * w},${h - (v / max) * (h - 2) - 1}`).join(' ');
    const area = `0,${h} ${pts} ${w},${h}`;
    return { w, h, pts, area };
  });
</script>

{#if view}
  <svg viewBox={`0 0 ${view.w} ${view.h}`} preserveAspectRatio="none" style:height={`${view.h}px`}>
    <polygon points={view.area} fill={color} opacity="0.12" />
    <polyline points={view.pts} fill="none" stroke={color} stroke-width="1.2" vector-effect="non-scaling-stroke" />
  </svg>
{/if}

<style>
  svg { width: 100%; display: block; }
</style>
