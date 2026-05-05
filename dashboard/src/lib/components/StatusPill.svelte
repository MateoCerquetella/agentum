<script lang="ts">
  import type { Status } from '$lib/api';

  interface Props {
    status: Status;
  }

  let { status }: Props = $props();

  const label: Record<Status, string> = {
    idle: 'idle',
    running: 'running',
    stopped: 'stopped',
    crashed: 'crashed'
  };
  const glyph: Record<Status, string> = {
    idle: '○',
    running: '●',
    stopped: '◇',
    crashed: '✕'
  };
</script>

<span class="pill" data-status={status} aria-label={`Session status: ${label[status]}`}>
  <span class="dot" aria-hidden="true">{glyph[status]}</span>
  <span class="label">{label[status]}</span>
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 0.4em;
    padding: 0.2em 0.55em;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    font-family: var(--font-mono);
    font-size: 0.78em;
    line-height: 1;
    letter-spacing: 0.02em;
    color: var(--text-2);
  }
  .pill[data-status="running"] { color: var(--success); border-color: color-mix(in srgb, var(--success) 35%, var(--border)); }
  .pill[data-status="crashed"] { color: var(--danger);  border-color: color-mix(in srgb, var(--danger) 35%, var(--border)); }
  .pill[data-status="idle"]    { color: var(--muted); }
  .dot { font-size: 1.1em; }
</style>
