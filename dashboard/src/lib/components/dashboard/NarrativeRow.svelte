<script lang="ts">
  /**
   * One bullet in the hero narrative. Tone color goes on the icon
   * chip; lead+body inherit the body color.
   */
  type Tone = 'ok' | 'warn' | 'crash' | 'info';
  interface Props {
    icon: string;
    tone: Tone;
    lead: string;
    body: string;
  }
  let { icon, tone, lead, body }: Props = $props();

  const colors: Record<Tone, string> = {
    ok: 'var(--green)',
    warn: 'var(--amber)',
    crash: 'var(--crash)',
    info: 'var(--blu)'
  };
</script>

<div class="row">
  <span class="chip" style:color={colors[tone]} style:background={`color-mix(in oklab, ${colors[tone]} 15%, transparent)`}>{icon}</span>
  <div class="text">
    <span class="lead">{lead}</span><span class="body">{body}</span>
  </div>
</div>

<style>
  .row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    font-size: 13.5px;
    line-height: 1.55;
    color: var(--fg-2);
  }
  .chip {
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    border-radius: var(--radius-md);
    display: inline-grid;
    place-items: center;
    font-size: 11px;
    font-weight: 600;
    margin-top: 1px;
  }
  .lead {
    color: var(--fg);
    font-weight: 500;
  }
</style>
