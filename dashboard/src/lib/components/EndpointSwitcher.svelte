<script lang="ts">
  /**
   * Topbar widget for switching between named agentum endpoints.
   *
   * Surfaces the current profile as a chip in the header; clicking it
   * opens a dropdown with the full list, an "Add…" form, and a remove
   * button per profile. Switching emits a custom event so callers can
   * decide how to refresh (the simplest correct thing is a full
   * page reload, which guarantees every store, WS, and cache reflects
   * the new endpoint without per-store invalidation logic).
   */
  import {
    profiles,
    activeProfileId,
    setActiveProfile,
    upsertProfile,
    removeProfile,
    type Profile
  } from '$lib/profiles';
  import {
    fleet,
    profileDisplayLabel,
    profileHostHint
  } from '$stores/fleet';

  let open = $state(false);
  let formOpen = $state(false);
  let formId = $state('');
  let formLabel = $state('');
  let formUrl = $state('');
  let formError = $state<string | null>(null);

  const active = $derived(
    $profiles.find((p) => p.id === $activeProfileId) ?? $profiles[0]
  );

  function dotClass(p: Profile): string {
    const e = $fleet[p.id];
    if (!e) return 'unknown';
    return e.status;
  }
  function rowHost(p: Profile): string {
    const hint = profileHostHint(p);
    if (hint) return hint;
    const hostname = $fleet[p.id]?.hostname;
    return hostname ? `${hostname} · this origin` : 'this origin';
  }

  function pick(id: string) {
    if (id === $activeProfileId) {
      open = false;
      return;
    }
    setActiveProfile(id);
    open = false;
    // The simplest correct way to swap every store, WS, and cache to
    // the new origin is a full reload — the alternative is per-store
    // re-init logic in every consumer, which is brittle and easy to
    // get wrong. The user's bearer token survives in localStorage so
    // the refresh lands them on the new server without re-login (if
    // they've logged into it before).
    if (typeof location !== 'undefined') location.reload();
  }

  function submitNew(e: SubmitEvent) {
    e.preventDefault();
    formError = null;
    const id = formId.trim();
    const label = formLabel.trim() || id;
    const url = formUrl.trim();
    if (!id) {
      formError = 'id is required';
      return;
    }
    try {
      // Reject malformed URLs eagerly so the next page load doesn't
      // silently fall back to the page origin.
      if (url) new URL(url);
    } catch {
      formError = 'invalid URL';
      return;
    }
    try {
      upsertProfile({ id, label, baseUrl: url, token: '' });
    } catch (e) {
      formError = e instanceof Error ? e.message : String(e);
      return;
    }
    formId = '';
    formLabel = '';
    formUrl = '';
    formOpen = false;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && open) {
      open = false;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="wrap" class:open>
  <button
    type="button"
    class="chip"
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={() => (open = !open)}
    title="Active agentum server — click to switch"
  >
    <span class="dot" class:unreachable={active && dotClass(active) === 'unreachable'}
                       class:login={active && dotClass(active) === 'login-needed'}></span>
    <span class="label">{active ? profileDisplayLabel(active, $fleet[active.id]) : 'this server'}</span>
    <span class="caret" aria-hidden="true">▾</span>
  </button>

  {#if open}
    <div class="menu" role="menu">
      <div class="menu-head">Servers</div>
      {#each $profiles as p (p.id)}
        <div class="row" class:on={p.id === $activeProfileId}>
          <button
            type="button"
            class="row-pick"
            onclick={() => pick(p.id)}
          >
            <span class="r-label">
              <span class="r-dot {dotClass(p)}"></span>
              {profileDisplayLabel(p, $fleet[p.id])}
              {#if $fleet[p.id]?.status === 'unreachable'}
                <span class="r-bad">unreachable</span>
              {:else if $fleet[p.id]?.status === 'login-needed'}
                <span class="r-warn">login needed</span>
              {/if}
            </span>
            <span class="r-host">{rowHost(p)}</span>
          </button>
          {#if $profiles.length > 1}
            <button
              type="button"
              class="row-rm"
              title={`Remove ${p.label}`}
              onclick={() => removeProfile(p.id)}
            >×</button>
          {/if}
        </div>
      {/each}

      <div class="sep"></div>

      {#if !formOpen}
        <button
          type="button"
          class="add-toggle"
          onclick={() => (formOpen = true)}
        >+ Add server</button>
      {:else}
        <form class="add" onsubmit={submitNew}>
          <input
            type="text"
            bind:value={formId}
            placeholder="id (e.g. vps)"
            autocomplete="off"
            spellcheck="false"
            required
          />
          <input
            type="text"
            bind:value={formLabel}
            placeholder="label (optional)"
            autocomplete="off"
            spellcheck="false"
          />
          <input
            type="url"
            bind:value={formUrl}
            placeholder="https://my-vps.example.com:8822"
            autocomplete="off"
            spellcheck="false"
          />
          {#if formError}
            <div class="err">{formError}</div>
          {/if}
          <div class="add-actions">
            <button type="button" class="ghost" onclick={() => (formOpen = false)}>Cancel</button>
            <button type="submit" class="primary">Save</button>
          </div>
        </form>
      {/if}
    </div>
  {/if}
</div>

<style>
  .wrap { position: relative; }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-pill);
    color: var(--fg-2);
    font-family: var(--mono);
    font-size: 11px;
    cursor: pointer;
    transition: border-color var(--t-hover), color var(--t-hover);
  }
  .chip:hover { color: var(--fg); border-color: var(--fg-3); }
  .open .chip { border-color: var(--cta); color: var(--fg); }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--green);
  }
  .dot.unreachable { background: var(--crash, #ff4d4f); }
  .dot.login { background: var(--warn, #d4a017); }
  .r-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--fg-3);
    margin-right: 4px;
    vertical-align: middle;
  }
  .r-dot.live { background: var(--green); }
  .r-dot.unreachable { background: var(--crash, #ff4d4f); }
  .r-dot.login-needed { background: var(--warn, #d4a017); }
  .r-dot.unknown { background: var(--fg-3); }
  .r-bad {
    margin-left: 6px;
    color: var(--crash, #ff4d4f);
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .r-warn {
    margin-left: 6px;
    color: var(--warn, #d4a017);
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .label { letter-spacing: -0.005em; }
  .caret { color: var(--fg-3); font-size: 9px; }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    min-width: 280px;
    background: var(--bg);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
    padding: 8px;
    z-index: 90;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .menu-head {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
    padding: 4px 8px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0;
  }
  .row.on .row-pick { background: color-mix(in srgb, var(--cta) 14%, var(--surface)); }
  .row-pick {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: 6px 8px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    cursor: pointer;
    text-align: left;
    transition: border-color var(--t-hover);
  }
  .row-pick:hover { border-color: var(--fg-3); }
  .r-label { font-size: 12.5px; }
  .r-host {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
  }
  .row-rm {
    width: 24px;
    height: 24px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg-3);
    font-size: 14px;
    cursor: pointer;
    transition: color var(--t-hover), border-color var(--t-hover);
  }
  .row-rm:hover { color: var(--crash); border-color: var(--crash); }

  .sep {
    height: 1px;
    background: var(--border);
    margin: 4px 0;
  }

  .add-toggle {
    background: transparent;
    border: 1px dashed var(--border-2);
    border-radius: var(--radius-sm);
    color: var(--fg-3);
    font-family: var(--mono);
    font-size: 11px;
    padding: 6px 8px;
    cursor: pointer;
    text-align: left;
  }
  .add-toggle:hover { color: var(--fg); border-color: var(--fg-3); }

  .add { display: flex; flex-direction: column; gap: 6px; }
  .add input {
    padding: 6px 8px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .add input:focus { outline: none; border-color: var(--cta); }
  .err {
    color: var(--crash);
    font-family: var(--mono);
    font-size: 11px;
  }
  .add-actions { display: flex; justify-content: flex-end; gap: 6px; }
  .add-actions button {
    padding: 5px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-2);
    background: var(--surface);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 11px;
    cursor: pointer;
  }
  .add-actions .primary { background: var(--cta); border-color: var(--cta); color: #fff; }
  .add-actions .primary:hover { filter: brightness(1.05); }
</style>
