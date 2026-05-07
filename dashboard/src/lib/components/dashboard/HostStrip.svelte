<script lang="ts">
  /**
   * Live CPU + RAM strip. Two sparkline tiles fed by the `host` store
   * which is itself driven by `host.metrics` events on the WS bus.
   * Color-coded thresholds: green <60%, amber <85%, red ≥85%.
   */
  import { host, fmtBytes } from '$stores/host';
  import Sparkline from './Sparkline.svelte';

  const cpu = $derived($host.cpu);
  const mem = $derived($host.memPct);
  const latest = $derived($host.latest);

  const cpuNow = $derived(latest ? Math.round(latest.cpu_pct) : 0);
  const memNow = $derived(
    latest && latest.mem_total > 0
      ? Math.round((latest.mem_used / latest.mem_total) * 100)
      : 0
  );

  function colorFor(pct: number): string {
    if (pct >= 85) return 'var(--cta)';
    if (pct >= 60) return 'var(--amber)';
    return 'var(--green)';
  }

  const cpuColor = $derived(colorFor(cpuNow));
  const memColor = $derived(colorFor(memNow));

  const ramLine = $derived(
    latest ? `${fmtBytes(latest.mem_used)} / ${fmtBytes(latest.mem_total)}` : '—'
  );
  const cpuLine = $derived(
    latest ? `${latest.cpu_count} core${latest.cpu_count === 1 ? '' : 's'}` : '—'
  );
</script>

<div class="strip">
  <div class="tile">
    <div class="row">
      <span class="k">CPU</span>
      <span class="v" style:color={cpuColor}>{cpuNow}%</span>
    </div>
    <Sparkline data={cpu} color={cpuColor} height={22} />
    <div class="sub">{cpuLine}</div>
  </div>
  <div class="tile">
    <div class="row">
      <span class="k">RAM</span>
      <span class="v" style:color={memColor}>{memNow}%</span>
    </div>
    <Sparkline data={mem} color={memColor} height={22} />
    <div class="sub">{ramLine}</div>
  </div>
</div>

<style>
  .strip {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    grid-column: 1 / -1;
  }
  .tile {
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-lg);
    padding: 10px 12px 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .k {
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .v {
    font-family: var(--display);
    font-size: 18px;
    line-height: 1;
    letter-spacing: -0.02em;
  }
  .sub {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
  }
</style>
