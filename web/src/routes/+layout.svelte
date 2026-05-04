<script lang="ts">
  import '../app.css';
  import Sidebar from '$components/Sidebar.svelte';
  import Topbar from '$components/Topbar.svelte';
  import { theme, applyTheme } from '$stores/theme';
  import { onMount } from 'svelte';

  interface Props { children: import('svelte').Snippet }
  let { children }: Props = $props();

  onMount(() => {
    // Re-apply on mount so the persisted theme wins over the hard-coded
    // app.html attribute.
    let current: typeof $theme = 'terminal-dark';
    theme.subscribe((t) => (current = t))();
    applyTheme(current);
  });
</script>

<div class="shell">
  <Sidebar />
  <div class="main">
    <Topbar />
    <main>{@render children()}</main>
  </div>
</div>

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
  }

  @media (max-width: 720px) {
    .shell { flex-direction: column; }
    main { padding: 1rem; }
  }
</style>
