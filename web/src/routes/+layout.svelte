<script lang="ts">
  import { get } from 'svelte/store';
  import '../app.css';
  import Sidebar from '$components/Sidebar.svelte';
  import Topbar from '$components/Topbar.svelte';
  import TokenGate from '$components/TokenGate.svelte';
  import { theme, applyTheme } from '$stores/theme';
  import { onMount } from 'svelte';

  interface Props { children: import('svelte').Snippet }
  let { children }: Props = $props();

  onMount(() => {
    // Re-apply on mount so the persisted theme wins over the hard-coded
    // app.html attribute.
    applyTheme(get(theme));
  });
</script>

<TokenGate>
  {#snippet children()}
    <div class="shell">
      <Sidebar />
      <div class="main">
        <Topbar />
        <main>{@render originalChildren()}</main>
      </div>
    </div>
  {/snippet}
</TokenGate>

{#snippet originalChildren()}
  {@render children()}
{/snippet}

<style>
  .shell {
    display: flex;
    min-height: 100vh;
  }
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  main {
    flex: 1;
    padding: 1.5rem 1.75rem;
    max-width: 1100px;
    width: 100%;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  @media (max-width: 720px) {
    .shell { flex-direction: column; }
    main { padding: 1rem; }
  }
</style>
