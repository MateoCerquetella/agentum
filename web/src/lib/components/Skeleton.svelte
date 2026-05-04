<script lang="ts">
  interface Props {
    rows?: number;
    height?: string;
    width?: string;
  }
  let { rows = 3, height = '5.2rem', width = '100%' }: Props = $props();
</script>

<div class="stack" aria-hidden="true">
  {#each Array(rows) as _, i}
    <div class="bar" style:height style:width style:--delay={`${i * 80}ms`}></div>
  {/each}
</div>

<style>
  .stack {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .bar {
    background: linear-gradient(
      90deg,
      var(--surface-2) 0%,
      color-mix(in srgb, var(--accent) 6%, var(--surface-2)) 50%,
      var(--surface-2) 100%
    );
    background-size: 200% 100%;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    animation: shimmer 1.6s ease-in-out infinite;
    animation-delay: var(--delay, 0ms);
    opacity: 0.85;
  }
  @keyframes shimmer {
    0%   { background-position: 200% 0; }
    100% { background-position: -200% 0; }
  }
</style>
