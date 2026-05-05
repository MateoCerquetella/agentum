<script lang="ts">
  interface Props {
    eyebrow?: string;
    title: string;
    body?: string;
    cmd?: string;
    cta?: { label: string; onclick: () => void };
  }
  let { eyebrow = 'Empty', title, body, cmd, cta }: Props = $props();
</script>

<div class="empty">
  <div class="inner">
    <div class="halo" aria-hidden="true"></div>
    <span class="eyebrow">{eyebrow}</span>
    <h2 class="display-2">{title}</h2>
    {#if body}<p class="body">{body}</p>{/if}
    {#if cmd}
      <pre class="cmd"><span class="prompt">$</span><code>{cmd}</code></pre>
    {/if}
    {#if cta}
      <button type="button" class="btn btn-cta" onclick={cta.onclick}>{cta.label}</button>
    {/if}
  </div>
</div>

<style>
  .empty {
    display: flex;
    justify-content: center;
    padding: 72px 24px 96px;
  }
  .inner {
    position: relative;
    max-width: 520px;
    width: 100%;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 56px 32px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .halo {
    position: absolute;
    inset: -1px;
    background: radial-gradient(
      ellipse 80% 50% at 50% 0%,
      color-mix(in srgb, var(--accent) 10%, transparent) 0%,
      transparent 70%
    );
    pointer-events: none;
  }
  .eyebrow { z-index: 1; }
  .display-2 {
    z-index: 1;
    color: var(--text);
  }
  .body {
    z-index: 1;
    margin: 0;
    color: var(--text-2);
    font-size: 15px;
    line-height: 1.5;
    max-width: 38ch;
  }
  .cmd {
    z-index: 1;
    margin: 4px 0 0;
    padding: 12px 16px;
    background: var(--bg);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 1;
    color: var(--text);
    display: inline-flex;
    align-items: center;
    gap: 10px;
  }
  .cmd .prompt { color: var(--muted); user-select: none; }
  .cmd code { color: var(--text); }
  .btn { z-index: 1; margin-top: 8px; }
</style>
