<script lang="ts">
  import RemoteAccessInfo from '$components/RemoteAccessInfo.svelte';
  import HostStrip from '$components/dashboard/HostStrip.svelte';
  import {
    tweaks, setAccent, setDensity, setTheme,
    setNotifyAwaitingInput, setNotifyFinished, setNotifyCrashed, setNotifyCompact,
    setNotifyBrowser, setHideHostStrip, setStuckMinutes,
    ACCENTS, DENSITIES
  } from '$stores/tweaks';
  import { THEMES } from '$stores/themes';

  const darkThemes  = THEMES.filter(t => t.mode === 'dark');
  const lightThemes = THEMES.filter(t => t.mode === 'light');
  import { host, fmtBytes } from '$stores/host';
  import {
    notifyPermission, isSupported as notifySupported,
    requestPermission as requestNotifyPermission, refreshPermission, notify
  } from '$stores/notify';
  import { onMount } from 'svelte';

  const latest = $derived($host.latest);

  onMount(() => {
    // Permission can change in browser settings while the SPA is open;
    // refresh on mount so the toggle reflects reality.
    refreshPermission();
  });

  // Toggling browser notifications on prompts for permission. If the
  // user denied previously the toggle is force-disabled with guidance
  // — we can't re-prompt once they've said no.
  async function onToggleBrowserNotify(next: boolean) {
    if (!next) { setNotifyBrowser(false); return; }
    if (!notifySupported()) { setNotifyBrowser(false); return; }
    if ($notifyPermission === 'denied') return;
    if ($notifyPermission !== 'granted') {
      const result = await requestNotifyPermission();
      if (result !== 'granted') { setNotifyBrowser(false); return; }
    }
    setNotifyBrowser(true);
  }

  function sendTestNotification() {
    notify({
      title: 'agentum',
      body: 'Browser notifications are working.',
      tag: 'test',
      urgent: true
    });
  }
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
          <span class="lbl-h">Theme</span>
          <span class="lbl-d">Reskins surfaces and foreground; the accent above stays independent. Picking a theme snaps the accent to its signature color — re-pick below to override.</span>
        </div>
        <div class="opts theme-grid">
          <span class="theme-group-lbl">Dark</span>
          {#each darkThemes as t (t.id)}
            <button
              type="button"
              class="theme-chip"
              class:active={$tweaks.theme === t.id}
              onclick={() => setTheme(t.id)}
              title={t.label}
              aria-label={`Use ${t.label} theme`}
            >
              <span class="theme-swatch" style:background={t.swatch}></span>
              <span class="theme-name">{t.label}</span>
            </button>
          {/each}
          <span class="theme-group-lbl">Light</span>
          {#each lightThemes as t (t.id)}
            <button
              type="button"
              class="theme-chip"
              class:active={$tweaks.theme === t.id}
              onclick={() => setTheme(t.id)}
              title={t.label}
              aria-label={`Use ${t.label} theme`}
            >
              <span class="theme-swatch" style:background={t.swatch}></span>
              <span class="theme-name">{t.label}</span>
            </button>
          {/each}
        </div>
      </div>

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
        <h2>Toast &amp; browser alerts</h2>
      </header>
      <p class="body-text">
        Each toggle gates a single event kind. In-app toasts appear in the
        corner while the tab is open; browser alerts pop the OS notification
        center when the dashboard is hidden or in another window.
      </p>

      <div class="row">
        <div class="lbl">
          <span class="lbl-h">Browser notifications</span>
          <span class="lbl-d">
            Mirror toast events to OS-level alerts so you get pinged when the
            dashboard is in another tab or behind another window. Requires
            granting the browser permission.
            {#if $notifyPermission === 'denied'}
              <strong style="color: var(--cta);"> Permission was denied — re-enable in your browser's site settings to flip this on.</strong>
            {:else if $notifyPermission === 'unsupported'}
              <strong style="color: var(--fg-3);"> This browser doesn't expose the Notifications API.</strong>
            {/if}
          </span>
        </div>
        <div class="opts" style="flex-direction: column; align-items: flex-end; gap: 6px;">
          <label class="switch" class:disabled={$notifyPermission === 'denied' || $notifyPermission === 'unsupported'}>
            <input
              type="checkbox"
              checked={$tweaks.notifyBrowser && $notifyPermission === 'granted'}
              disabled={$notifyPermission === 'denied' || $notifyPermission === 'unsupported'}
              onchange={(e) => onToggleBrowserNotify((e.currentTarget as HTMLInputElement).checked)}
            />
            <span class="track"><span class="thumb"></span></span>
          </label>
          {#if $tweaks.notifyBrowser && $notifyPermission === 'granted'}
            <button type="button" class="ghost-btn" onclick={sendTestNotification}>
              Send test
            </button>
          {/if}
        </div>
      </div>

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

  /* Theme picker — wraps so it doesn't overflow the settings column. */
  .theme-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
    justify-content: flex-end;
    max-width: 520px;
  }
  .theme-group-lbl {
    width: 100%;
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-3);
    margin-top: 4px;
    text-align: right;
  }
  .theme-group-lbl:first-child { margin-top: 0; }
  .theme-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px 4px 6px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border-2);
    background: var(--bg-2);
    color: var(--fg-2);
    font-size: 11.5px;
    font-family: var(--mono);
    cursor: pointer;
    transition: color var(--t-hover), border-color var(--t-hover), background var(--t-hover);
  }
  .theme-chip:hover { color: var(--fg); border-color: var(--fg-3); }
  .theme-chip.active {
    color: var(--fg);
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 14%, var(--bg-2));
  }
  .theme-swatch {
    width: 14px;
    height: 14px;
    border-radius: var(--radius-pill);
    border: 1px solid color-mix(in srgb, var(--fg) 18%, transparent);
    flex-shrink: 0;
  }
  .theme-name { white-space: nowrap; }

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
  .switch.disabled { cursor: not-allowed; opacity: 0.45; }
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

  /* Subtle action button used inline beside switches. */
  .ghost-btn {
    padding: 4px 10px;
    border-radius: var(--radius);
    border: 1px solid var(--border-2);
    background: var(--bg-2);
    color: var(--fg-2);
    font-family: var(--mono);
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
    transition: color var(--t-hover), border-color var(--t-hover), background var(--t-hover);
  }
  .ghost-btn:hover {
    color: var(--fg);
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 10%, var(--bg-2));
  }

  /* Stack the row above ~640px so labels never crash into the toggles
     and so long descriptions read in a comfortable column on phones.
     On desktop the original two-column grid stays intact. */
  @media (max-width: 720px) {
    /* Sticky route header so "Settings" stays visible while scrolling. */
    :global(.toolbar) {
      position: sticky;
      top: 0;
      z-index: 5;
      background: color-mix(in srgb, var(--bg-chrome) 92%, transparent);
      backdrop-filter: blur(10px);
      -webkit-backdrop-filter: blur(10px);
    }
    .scroll {
      padding: 14px 12px 28px;
      gap: 14px;
    }
    .block {
      padding: 16px 14px;
      border-radius: 12px;
    }
    /* Native iOS Settings feel: label hugs left, control hugs right.
       Multi-row controls (swatches, segmented) opt back into stacking
       via the row.row-stack helper class. */
    .row {
      grid-template-columns: minmax(0, 1fr) auto;
      align-items: center;
      gap: 12px;
    }
    /* When the description is long, drop a second-row span; this is
       just a wrapping fallback handled by .lbl-d's natural width. */
    .opts { justify-content: flex-end; }
    .seg { flex-wrap: wrap; }
    .seg-opt { flex: 1 1 auto; min-height: 36px; padding: 8px 12px; }
    .swatch { width: 32px; height: 32px; }
    .num { width: 84px; padding: 9px 10px; font-size: 13px; }

    /* iOS-size toggles. */
    .switch .track { width: 46px; height: 28px; border-radius: 999px; }
    .switch .thumb { width: 22px; height: 22px; top: 2px; left: 2px; }
    .switch input:checked + .track .thumb { left: 21px; }

    .lbl-h { font-size: 14px; }
    .lbl-d { font-size: 12px; max-width: none; }
    .block-h h2 { font-size: 16px; }

    .kv {
      grid-template-columns: 1fr 1fr;
    }
  }
  /* Stack the row entirely on the smallest phones so the label can
     breathe and the swatch row falls underneath. */
  @media (max-width: 480px) {
    .row {
      grid-template-columns: 1fr;
      align-items: flex-start;
    }
    .opts { justify-content: flex-start; width: 100%; }
  }
  @media (max-width: 380px) {
    .kv { grid-template-columns: 1fr; }
  }
</style>
