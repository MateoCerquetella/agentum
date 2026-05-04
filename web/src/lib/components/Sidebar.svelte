<script lang="ts">
  import { page } from '$app/state';

  const items = [
    { href: '/',         label: 'Sessions' },
    { href: '/board',    label: 'Board' },
    { href: '/notes',    label: 'Notes',    soon: true },
    { href: '/channels', label: 'Channels', soon: true },
    { href: '/settings', label: 'Settings', soon: true }
  ];

  function isActive(href: string): boolean {
    const p = page.url.pathname;
    if (href === '/') return p === '/' || p.startsWith('/sessions');
    return p === href || p.startsWith(href + '/');
  }
</script>

<aside class="sidebar">
  <a class="brand" href="/">
    <span class="mark mono">⟁</span>
    <span class="name">agentum</span>
  </a>

  <nav>
    {#each items as item}
      <a
        class="nav-item"
        class:active={isActive(item.href)}
        class:soon={item.soon}
        href={item.href}
        aria-current={isActive(item.href) ? 'page' : undefined}
      >
        <span>{item.label}</span>
        {#if item.soon}<span class="badge">soon</span>{/if}
      </a>
    {/each}
  </nav>

  <footer class="muted">
    <span>v{__APP_VERSION__}</span>
  </footer>
</aside>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 1rem 0.75rem;
    width: 220px;
    min-width: 220px;
    border-right: 1px solid var(--border);
    background: var(--surface);
    height: 100vh;
    position: sticky;
    top: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 0.6rem 0.9rem;
    border-bottom: 1px solid var(--border);
    color: var(--text);
  }
  .mark {
    color: var(--accent);
    font-size: 1.4rem;
    line-height: 1;
  }
  .name {
    font-family: var(--font-display);
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    margin-top: 0.4rem;
    flex: 1;
  }
  .nav-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem 0.7rem;
    border-radius: 6px;
    color: var(--text-2);
  }
  .nav-item:hover { background: var(--surface-2); color: var(--text); }
  .nav-item.active {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    color: var(--text);
  }
  .nav-item.soon { color: var(--muted); }
  .badge {
    font-size: 0.7rem;
    padding: 0.05em 0.45em;
    border-radius: 999px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--muted);
    font-family: var(--font-mono);
  }
  footer {
    padding: 0.6rem 0.7rem;
    font-family: var(--font-mono);
    font-size: 0.75rem;
  }
  .mono { font-family: var(--font-mono); }
  .muted { color: var(--muted); }

  @media (max-width: 720px) {
    .sidebar {
      width: 100%;
      min-width: 0;
      height: auto;
      flex-direction: row;
      position: static;
      align-items: center;
      gap: 0.75rem;
      padding: 0.6rem;
    }
    nav { flex-direction: row; overflow-x: auto; flex: 1; }
    .brand { padding: 0.4rem 0.6rem; border-bottom: 0; border-right: 1px solid var(--border); }
    footer { display: none; }
  }
</style>
