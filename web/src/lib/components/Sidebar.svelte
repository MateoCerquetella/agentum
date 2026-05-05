<script lang="ts">
  import { page } from '$app/state';
  import { sessions } from '$stores/sessions';
  import { get } from 'svelte/store';
  import Icon from './Icon.svelte';

  interface NavItem {
    href: string;
    label: string;
    icon: string;
    soon?: boolean;
  }

  const items: NavItem[] = [
    { href: '/',          label: 'Agents',    icon: 'monitor' },
    { href: '/terminals', label: 'Terminals', icon: 'terminal' },
    { href: '/settings',  label: 'Settings',  icon: 'settings' }
  ];

  function isActive(href: string): boolean {
    const p = page.url.pathname;
    if (href === '/') {
      // Sessions list + per-session pages light up "Agents".
      // /terminals has its own nav entry, so it must NOT match here.
      return p === '/' || p.startsWith('/sessions');
    }
    return p === href || p.startsWith(href + '/');
  }

  let runningCount = $state(0);
  $effect(() => {
    const v = get(sessions);
    runningCount = v.items.filter(s => s.status === 'running').length;
  });
</script>

<aside class="sidebar">
  <a class="brand" href="/">
    <span class="logo" aria-hidden="true">
      <Icon name="cpu" size={22} />
    </span>
    <span class="name">
      <span class="dot" class:alive={runningCount > 0}></span>
      agentum
    </span>
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
        <Icon name={item.icon} size={16} />
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
    gap: 0.25rem;
    padding: 0.75rem 0.5rem;
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
    gap: 0.5rem;
    padding: 0.5rem 0.6rem 0.7rem;
    margin-bottom: 0.25rem;
    border-bottom: 1px solid var(--border);
    color: var(--text);
    text-decoration: none;
    transition: opacity var(--transition, 150ms ease);
  }
  .brand:hover { opacity: 0.85; }
  .logo {
    color: var(--accent);
    display: flex;
    align-items: center;
  }
  .name {
    font-family: var(--font-display);
    font-weight: 600;
    font-size: 0.95rem;
    letter-spacing: -0.01em;
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--muted);
    display: inline-block;
    flex-shrink: 0;
    transition: background var(--transition, 150ms ease);
  }
  .dot.alive { background: var(--success); box-shadow: 0 0 4px var(--success); }
  nav {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    flex: 1;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    padding: 0.45rem 0.6rem;
    border-radius: 6px;
    color: var(--text-2);
    text-decoration: none;
    font-size: 0.85rem;
    transition: background var(--transition, 150ms ease), color var(--transition, 150ms ease);
    position: relative;
  }
  .nav-item:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .nav-item.active {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--text);
    font-weight: 500;
  }
  .nav-item.active :global(.icon) {
    opacity: 1;
    color: var(--accent);
  }
  .nav-item.soon { color: var(--muted); cursor: default; }
  .nav-item.soon:hover { background: transparent; color: var(--muted); }
  .badge {
    font-size: 0.65rem;
    padding: 0.05em 0.4em;
    border-radius: 999px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--muted);
    font-family: var(--font-mono);
    margin-left: auto;
  }
  footer {
    padding: 0.5rem 0.6rem;
    font-family: var(--font-mono);
    font-size: 0.72rem;
    border-top: 1px solid var(--border);
    margin-top: 0.25rem;
  }
  .muted { color: var(--muted); }

  @media (max-width: 720px) {
    .sidebar {
      width: 100%;
      min-width: 0;
      height: auto;
      flex-direction: row;
      position: static;
      align-items: center;
      gap: 0.5rem;
      padding: 0.5rem;
      border-right: 0;
      border-bottom: 1px solid var(--border);
    }
    nav {
      flex-direction: row;
      overflow-x: auto;
      flex: 1;
    }
    .brand { padding: 0.3rem 0.5rem; border-bottom: 0; border-right: 1px solid var(--border); margin-bottom: 0; }
    .nav-item { white-space: nowrap; padding: 0.35rem 0.5rem; font-size: 0.78rem; }
    .badge { display: none; }
    footer { display: none; }
  }
</style>
