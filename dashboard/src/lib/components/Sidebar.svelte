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
    if (href === '/') return p === '/' || p.startsWith('/sessions');
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
      <Icon name="cpu" size={20} />
    </span>
    <span class="wordmark">
      <span class="status-dot" data-status={runningCount > 0 ? 'running' : 'idle'}></span>
      <span class="name">agentum</span>
    </span>
  </a>

  <div class="nav-group">
    <span class="eyebrow nav-heading">Navigation</span>
    <nav>
      {#each items as item}
        <a
          class="nav-item"
          class:active={isActive(item.href)}
          class:soon={item.soon}
          href={item.href}
          aria-current={isActive(item.href) ? 'page' : undefined}
        >
          <span class="rail" aria-hidden="true"></span>
          <Icon name={item.icon} size={15} />
          <span class="label">{item.label}</span>
          {#if item.soon}<span class="badge mono">soon</span>{/if}
        </a>
      {/each}
    </nav>
  </div>

  <footer>
    <span class="eyebrow">Build</span>
    <span class="ver mono">v{__APP_VERSION__}</span>
  </footer>
</aside>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    width: 224px;
    min-width: 224px;
    border-right: 1px solid var(--border);
    background: var(--surface);
    height: 100vh;
    position: sticky;
    top: 0;
    padding: 18px 14px;
    gap: 28px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 4px 6px 18px;
    border-bottom: 1px solid var(--border-2);
    color: var(--text);
    text-decoration: none;
    transition: opacity 120ms ease;
  }
  .brand:hover { opacity: 0.85; }
  .logo {
    color: var(--text);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    background: var(--bg);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-sm);
  }
  .wordmark {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .name {
    font-family: var(--font-display);
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.02em;
    color: var(--text);
  }

  .nav-group { display: flex; flex-direction: column; gap: 10px; flex: 1; }
  .nav-heading { padding: 0 8px; }

  nav { display: flex; flex-direction: column; gap: 2px; }

  .nav-item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    color: var(--text-2);
    text-decoration: none;
    font-size: 13.5px;
    transition: background 120ms ease, color 120ms ease;
  }
  .rail {
    position: absolute;
    left: -14px;
    top: 50%;
    transform: translateY(-50%);
    width: 2px;
    height: 0;
    background: var(--cta);
    transition: height 160ms cubic-bezier(0.2, 0.7, 0.2, 1);
  }
  .nav-item:hover {
    background: var(--bg);
    color: var(--text);
  }
  .nav-item.active {
    color: var(--text);
    background: var(--bg);
  }
  .nav-item.active .rail { height: 18px; }
  .nav-item.active :global(.icon) {
    color: var(--cta);
  }
  .label { letter-spacing: -0.005em; }
  .nav-item.soon { color: var(--muted); cursor: default; }
  .nav-item.soon:hover { background: transparent; color: var(--muted); }

  .badge {
    margin-left: auto;
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    padding: 2px 6px;
    border-radius: 99999px;
    background: var(--surface-2);
    color: var(--muted);
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 8px 0;
    border-top: 1px solid var(--border-2);
  }
  .ver { font-size: 11px; color: var(--muted); }

  @media (max-width: 720px) {
    .sidebar {
      width: 100%;
      min-width: 0;
      height: auto;
      flex-direction: row;
      position: static;
      align-items: center;
      gap: 10px;
      padding: 8px 10px;
      border-right: 0;
      border-bottom: 1px solid var(--border);
    }
    .brand { padding: 0 8px 0 0; border-bottom: 0; border-right: 1px solid var(--border-2); }
    .nav-group { flex-direction: row; gap: 0; }
    .nav-heading { display: none; }
    nav { flex-direction: row; overflow-x: auto; }
    .rail { display: none; }
    .nav-item { white-space: nowrap; padding: 6px 10px; font-size: 12.5px; }
    .badge { display: none; }
    footer { display: none; }
  }
</style>
