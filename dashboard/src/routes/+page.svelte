<script lang="ts">
  /**
   * The kanban board is the dashboard's main surface — drop visitors
   * straight onto /board. The legacy hero (greeting, fleet rows, host
   * metrics) is preserved at /home in case the user wants the old
   * shape back without a git checkout.
   */
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';

  onMount(() => {
    void goto('/board', { replaceState: true });
  });
</script>

<div class="redirect">
  <span class="mono">→ /board</span>
</div>

<style>
  .redirect {
    flex: 1;
    display: grid;
    place-items: center;
    color: var(--fg-3);
    font-family: var(--mono);
    font-size: 12px;
  }
</style>
