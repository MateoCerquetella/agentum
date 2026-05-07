<script lang="ts">
  import RemoteAccessInfo from '$components/RemoteAccessInfo.svelte';
  import HostStrip from '$components/dashboard/HostStrip.svelte';
  import {
    tweaks, setAccent, setDensity,
    setNotifyAwaitingInput, setNotifyFinished, setNotifyCrashed, setNotifyCompact,
    setHideHostStrip, setStuckMinutes,
    ACCENTS, DENSITIES
  } from '$stores/tweaks';
  import { host, fmtBytes } from '$stores/host';

  const latest = $derived($host.latest);
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

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Hide host strip</span>
          <span class="lbl-d">Removes the live CPU/RAM sparkline pair from the dashboard hero. The /api/host/metrics stream stays connected.</span>
        </div>
        <div class="opts">
          <label class="switch">
            <input type="checkbox" checked={$tweaks.hideHostStrip} onchange={(e) => setHideHostStrip((e.currentTarget as HTMLInputElement).checked)} />
            <span class="track"><span class="thumb"></span></span>
          </label>
        </div>
      </div>
    </section>

    <section class="block">
      <header class="block-h">
        <span class="micro">Live host</span>
        <h2>VPS metrics</h2>
      </header>
      <p class="body-text">
        Real-time view of the daemon host. CPU is averaged across logical cores;
        RAM excludes available pages so the value tracks "what's actually in use."
      </p>
      <div class="metrics-card">
        <HostStrip />
      </div>
      {#if latest}
        <dl class="kv">
          <div><dt>Cores</dt><dd>{latest.cpu_count}</dd></div>
          <div><dt>RAM</dt><dd>{fmtBytes(latest.mem_used)} / {fmtBytes(latest.mem_total)}</dd></div>
          <div><dt>Swap</dt><dd>{fmtBytes(latest.swap_used)} / {fmtBytes(latest.swap_total)}</dd></div>
          <div><dt>Per-core (peak)</dt><dd>{Math.max(0, ...latest.cores).toFixed(0)}%</dd></div>
        </dl>
      {:else}
        <p class="muted-mini">Waiting for first sample…</p>
      {/if}
    </section>

    <section class="block">
      <header class="block-h">
        <span class="micro">Notifications</span>
        <h2>Toast preferences</h2>
      </header>
      <p class="body-text">
        Each toggle gates a single event kind. Toasts only appear while the
        dashboard tab is open — desktop / OS notifications land in a follow-up.
      </p>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Awaiting input</span>
          <span class="lbl-d"><code>agent.awaiting_input</code> — agent is blocked on a permission prompt.</span>
        </div>
        <div class="opts">
          <label class="switch">
            <input type="checkbox" checked={$tweaks.notifyAwaitingInput} onchange={(e) => setNotifyAwaitingInput((e.currentTarget as HTMLInputElement).checked)} />
            <span class="track"><span class="thumb"></span></span>
          </label>
        </div>
      </div>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Agent finished</span>
          <span class="lbl-d"><code>agent.finished</code> — busy spinner cleared. Suppressed automatically when you're already viewing the session.</span>
        </div>
        <div class="opts">
          <label class="switch">
            <input type="checkbox" checked={$tweaks.notifyFinished} onchange={(e) => setNotifyFinished((e.currentTarget as HTMLInputElement).checked)} />
            <span class="track"><span class="thumb"></span></span>
          </label>
        </div>
      </div>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Crashes</span>
          <span class="lbl-d"><code>session.crashed</code> — pane killed by a panic / signal.</span>
        </div>
        <div class="opts">
          <label class="switch">
            <input type="checkbox" checked={$tweaks.notifyCrashed} onchange={(e) => setNotifyCrashed((e.currentTarget as HTMLInputElement).checked)} />
            <span class="track"><span class="thumb"></span></span>
          </label>
        </div>
      </div>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Auto-compact</span>
          <span class="lbl-d"><code>watchdog.compact</code> — watchdog issued <code>/compact</code> on a low-context session.</span>
        </div>
        <div class="opts">
          <label class="switch">
            <input type="checkbox" checked={$tweaks.notifyCompact} onchange={(e) => setNotifyCompact((e.currentTarget as HTMLInputElement).checked)} />
            <span class="track"><span class="thumb"></span></span>
          </label>
        </div>
      </div>
    </section>

    <section class="block">
      <header class="block-h">
        <span class="micro">Fleet</span>
        <h2>Attention thresholds</h2>
      </header>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Stuck after</span>
          <span class="lbl-d">Minutes a running session must sit idle (no activity) before the dashboard surfaces it in the "Needs attention" panel.</span>
        </div>
        <div class="opts">
          <input
            type="number"
            min="1"
            max="120"
            class="num"
            value={$tweaks.stuckMinutes}
            oninput={(e) => setStuckMinutes(Number((e.currentTarget as HTMLInputElement).value))}
          />
          <span class="muted-mini">min</span>
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
        Session defaults are still configured via
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

  /* iOS-style toggle. */
  .switch { display: inline-flex; cursor: pointer; }
  .switch input { display: none; }
  .switch .track {
    width: 36px;
    height: 20px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: 999px;
    position: relative;
    transition: background var(--t-hover), border-color var(--t-hover);
  }
  .switch .thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--fg-3);
    transition: left var(--t-hover), background var(--t-hover);
  }
  .switch input:checked + .track {
    background: color-mix(in oklab, var(--cta) 25%, var(--bg-2));
    border-color: var(--cta);
  }
  .switch input:checked + .track .thumb {
    left: 18px;
    background: var(--cta);
  }

  .num {
    width: 70px;
    padding: 5px 8px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 12px;
  }
  .num:focus { outline: 1px solid var(--cta); border-color: var(--cta); }

  .metrics-card {
    background: var(--bg-2);
    border-radius: var(--radius);
    padding: 8px;
  }

  .kv {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 10px;
    margin: 0;
    padding: 0;
  }
  .kv > div {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 12px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .kv dt {
    font-family: var(--mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-3);
  }
  .kv dd {
    margin: 0;
    font-family: var(--mono);
    font-size: 13px;
    color: var(--fg);
  }
  .muted-mini { color: var(--fg-3); font-family: var(--mono); font-size: 11px; }
</style>
