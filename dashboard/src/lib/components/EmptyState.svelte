<script lang="ts">
  interface Props {
    title: string;
    body?: string;
    cmd?: string;
  }
  let { title, body, cmd }: Props = $props();
</script>

<div class="empty">
  <h2>{title}</h2>
  {#if body}<p>{body}</p>{/if}
  {#if cmd}<pre><code>{cmd}</code></pre>{/if}
</div>

<style>
  .empty {
    text-align: center;
    padding: 3.5rem 1.5rem;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-2);
  }
  h2 {
    margin: 0 0 0.5rem;
    font-family: var(--font-display);
    font-weight: 600;
    color: var(--text);
    font-size: 1.25rem;
  }
  p { margin: 0 0 1rem; }
  pre {
    display: inline-block;
    text-align: left;
    margin: 0;
    padding: 0.7rem 1rem;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 0.85rem;
    color: var(--accent);
  }
</style>
