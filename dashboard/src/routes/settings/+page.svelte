<script lang="ts">
  import RemoteAccessInfo from '$components/RemoteAccessInfo.svelte';
  import { tweaks, setAccent, setDensity, ACCENTS, DENSITIES } from '$stores/tweaks';
</script>

<div class="page">
  <div class="toolbar">
    <span class="micro" style="color: var(--fg-2);">Settings</span>
    <span class="spacer"></span>
  </div>

  <div class="scroll">
    <section class="block">
      <header class="block-h">
        <span class="micro">Appearance</span>
        <h2>Theme &amp; density</h2>
      </header>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Accent</span>
          <span class="lbl-d">Brand color used for primary CTAs, focus rings, and the watchdog "compact" tone.</span>
        </div>
        <div class="opts">
          {#each ACCENTS as a (a.hex)}
            <button
              type="button"
              class="swatch"
              class:active={$tweaks.accent === a.hex}
              style:background={a.hex}
              onclick={() => setAccent(a.hex)}
              title={`${a.label} · ${a.hex}`}
              aria-label={`Use ${a.label} accent`}
            >
              {#if $tweaks.accent === a.hex}
                <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M3.5 8l3 3 6-6"/>
                </svg>
              {/if}
            </button>
          {/each}
        </div>
      </div>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Density</span>
          <span class="lbl-d">Root font size — propagates to spacing and component dimensions.</span>
        </div>
        <div class="opts seg">
          {#each DENSITIES as d (d.id)}
            <button
              type="button"
              class="seg-opt"
              class:active={$tweaks.density === d.id}
              onclick={() => setDensity(d.id)}
            >
              {d.label}
              <span class="seg-meta">{d.px}px</span>
            </button>
          {/each}
        </div>
      </div>
    </section>

    <section class="block">
      <header class="block-h">
        <span class="micro">Connectivity</span>
        <h2>Remote access</h2>
      </header>
      <p class="body-text">
        Share the URL and verify the fingerprint with whoever you want to grant
        access. Anyone who can reach this host on the network plus a valid login
        can use agentum.
      </p>
      <RemoteAccessInfo />
    </section>

    <section class="block dim">
      <header class="block-h">
        <span class="micro">CLI</span>
        <h2>More settings</h2>
      </header>
      <p class="body-text">
        Session defaults and notification preferences are still configured via
        <code>agentum config get | set | edit</code> on the host. Surfacing
        them here lands in a follow-up phase.
      </p>
    </section>
  </div>
</div>

<style>
  .page {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 24px 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 920px;
    width: 100%;
    margin: 0 auto;
  }

  .block {
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: 10px;
    padding: 18px 22px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .block.dim { opacity: 0.7; }

  .block-h h2 {
    margin: 4px 0 0;
    font-family: var(--display);
    font-size: 18px;
    font-weight: 500;
    letter-spacing: -0.015em;
    color: var(--fg);
  }

  .body-text {
    margin: 0;
    color: var(--fg-2);
    font-size: 13px;
    line-height: 1.55;
  }
  code {
    font-family: var(--mono);
    color: var(--fg);
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-sm);
    padding: 1px 6px;
    font-size: 12px;
  }

  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 16px;
    align-items: center;
    padding: 12px 0;
    border-top: 1px solid var(--border);
  }
  .row:first-of-type { border-top: 0; padding-top: 4px; }

  .lbl-h {
    display: block;
    color: var(--fg);
    font-size: 13.5px;
    letter-spacing: -0.005em;
  }
  .lbl-d {
    display: block;
    color: var(--fg-3);
    font-size: 11.5px;
    margin-top: 4px;
    line-height: 1.45;
    max-width: 60ch;
  }

  .opts { display: inline-flex; gap: 8px; align-items: center; }

  .swatch {
    width: 26px;
    height: 26px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border-2);
    cursor: pointer;
    display: inline-grid;
    place-items: center;
    transition: border-color var(--t-hover), transform var(--t-transform);
  }
  .swatch:hover { border-color: var(--fg-3); }
  .swatch.active {
    border-color: #fff;
    transform: scale(1.05);
  }

  .seg {
    display: inline-flex;
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    overflow: hidden;
    background: var(--bg-2);
  }
  .seg-opt {
    padding: 6px 12px;
    background: transparent;
    border: 0;
    color: var(--fg-3);
    font-family: var(--mono);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border-right: 1px solid var(--border-2);
  }
  .seg-opt:last-child { border-right: 0; }
  .seg-opt:hover { color: var(--fg-2); }
  .seg-opt.active { background: var(--surface); color: var(--fg); }
  .seg-meta { color: var(--fg-3); font-size: 9.5px; }
  .seg-opt.active .seg-meta { color: var(--fg-2); }
</style>
