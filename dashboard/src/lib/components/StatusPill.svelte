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
</script>

<span class="pill" data-status={status} aria-label={`Session status: ${label[status]}`}>
  <span class="status-dot" data-status={status} aria-hidden="true"></span>
  <span class="label">{label[status]}</span>
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px 4px 8px;
    border: 1px solid var(--border-2);
    border-radius: 99999px;
    background: transparent;
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 500;
    line-height: 1;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-2);
  }
  .pill[data-status="running"] {
    color: var(--success);
    border-color: color-mix(in srgb, var(--success) 35%, var(--border-2));
  }
  .pill[data-status="crashed"] {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 35%, var(--border-2));
  }
  .pill[data-status="stopped"] { color: var(--muted); }
  .pill[data-status="idle"]    { color: var(--muted); }
</style>
