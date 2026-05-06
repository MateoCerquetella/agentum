<script lang="ts">
  /**
   * Actionable callout pinned above the Fleet table. Each strip owns
   * one signal — a crashed pane, a low-ctx session, a PR awaiting
   * review — and carries its action through the `action` callback.
   */
  type Tone = 'crash' | 'warn' | 'amber' | 'info';
  interface Props {
    tone: Tone;
    label: string;
    target: string;
    detail: string;
    actionLabel: string;
    onAction: () => void;
    onDismiss?: () => void;
  }
  let { tone, label, target, detail, actionLabel, onAction, onDismiss }: Props = $props();

  const tones: Record<Tone, { c: string; bg: string; br: string }> = {
    crash: { c: 'var(--crash)', bg: 'rgba(255,85,85,0.07)',  br: 'rgba(255,85,85,0.30)' },
    warn:  { c: 'var(--cta)',   bg: 'rgba(243,100,88,0.07)', br: 'rgba(243,100,88,0.30)' },
    amber: { c: 'var(--amber)', bg: 'rgba(255,180,84,0.07)', br: 'rgba(255,180,84,0.28)' },
    info:  { c: 'var(--blu)',   bg: 'rgba(85,190,255,0.07)', br: 'rgba(85,190,255,0.30)' }
  };
  const t = $derived(tones[tone]);
</script>

<div class="strip" style:background={t.bg}>
  <span class="label" style:color={t.c} style:border-color={t.br}>{label}</span>
  <span class="target">{target}</span>
  <span class="detail">{detail}</span>
  <button type="button" class="action" style:color={t.c} style:border-color={t.br} onclick={onAction}>
    {actionLabel} →
  </button>
  {#if onDismiss}
    <button type="button" class="dismiss" onclick={onDismiss} aria-label="Dismiss">×</button>
  {/if}
</div>

<style>
  .strip {
    flex: 1;
    min-width: 0;
    padding: 10px 16px;
    display: flex;
    align-items: center;
    gap: 12px;
    border-right: 1px solid var(--border);
  }
  .strip:last-child { border-right: 0; }

  .label {
    flex-shrink: 0;
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 2px 7px;
    border-radius: var(--radius-sm);
    border: 1px solid;
    background: var(--bg-2);
  }
  .target {
    flex-shrink: 0;
    font-size: 13px;
    color: var(--fg);
    letter-spacing: -0.005em;
  }
  .detail {
    flex: 1;
    min-width: 0;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .action {
    flex-shrink: 0;
    padding: 4px 10px;
    border-radius: 4px;
    background: var(--bg-2);
    border: 1px solid;
    font-size: 10.5px;
    font-family: var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
  }
  .dismiss {
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    border-radius: var(--radius-sm);
    background: transparent;
    border: 0;
    color: var(--fg-3);
    cursor: pointer;
    font-size: 16px;
    line-height: 1;
  }
  .dismiss:hover { color: var(--fg); background: var(--surface); }
</style>
