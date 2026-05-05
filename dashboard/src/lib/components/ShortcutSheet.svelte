<script lang="ts">
  import { shortcuts, closeShortcuts } from '$stores/palette';

  const groups: Array<[string, Array<[string, string]>]> = [
    ['Global', [
      ['⌘ K  /  Ctrl+K', 'Open command palette'],
      ['?', 'Show this shortcut sheet'],
      ['Esc', 'Close any overlay']
    ]],
    ['Navigation', [
      ['Click sidebar', 'Switch view'],
      ['Click session card', 'Open session detail']
    ]],
    ['Sessions', [
      ['Type + Enter', 'Send text to pane'],
      ['^C button', 'Send Ctrl-C signal']
    ]],
    ['Notes', [
      ['Type', 'Auto-save after 800 ms idle'],
      ['Blur editor', 'Force save now']
    ]],
    ['Board', [
      ['Drag card', 'Move between columns'],
      ['Click claim', 'Atomic CAS — first wins']
    ]]
  ];
</script>

{#if $shortcuts.open}
  <div class="backdrop" role="presentation" onclick={closeShortcuts}></div>
  <div class="sheet" role="dialog" aria-label="Keyboard shortcuts">
    <header>
      <h3>Keyboard shortcuts</h3>
      <button class="x" type="button" onclick={closeShortcuts} title="close">×</button>
    </header>
    <div class="content">
      {#each groups as [section, rows]}
        <section class="group">
          <h4>{section}</h4>
          <dl>
            {#each rows as [keys, desc]}
              <dt><kbd>{keys}</kbd></dt>
              <dd>{desc}</dd>
            {/each}
          </dl>
        </section>
      {/each}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg) 65%, transparent);
    backdrop-filter: blur(2px);
    z-index: 60;
  }
  .sheet {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(540px, calc(100% - 2rem));
    max-height: 80vh;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 18px 60px rgba(0,0,0,0.35);
    z-index: 61;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.85rem 1rem;
    border-bottom: 1px solid var(--border);
  }
  h3 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 1.05rem;
    color: var(--text);
  }
  .x {
    font-size: 1.4rem;
    color: var(--muted);
    line-height: 1;
    padding: 0 0.4rem;
    border-radius: 4px;
  }
  .x:hover { color: var(--text); background: var(--surface-2); }

  .content {
    overflow-y: auto;
    padding: 0.6rem 1rem 1rem;
  }
  .group { margin-top: 0.6rem; }
  .group h4 {
    margin: 0.6rem 0 0.4rem;
    font-family: var(--font-mono);
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--muted);
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    margin: 0;
  }
  dt { display: flex; align-items: center; }
  kbd {
    font-family: var(--font-mono);
    font-size: 0.78rem;
    background: var(--surface-2);
    color: var(--text);
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 4px;
    padding: 0.05em 0.5em;
    white-space: nowrap;
  }
  dd {
    margin: 0;
    color: var(--text-2);
    font-size: 0.88rem;
    align-self: center;
  }
</style>
