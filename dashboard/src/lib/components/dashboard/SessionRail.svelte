<script lang="ts">
  import type { Session, WatchdogEvent } from '$lib/api';
  import { deriveState, ctxOf, fmtTokens, fmtCost, fmtUptime, toolShort } from '$lib/dashboard';
  import Watchdog from './Watchdog.svelte';
  import DiffBlock from './DiffBlock.svelte';
  import type { DiffHunk } from './types';

  /**
   * Right rail for the session-detail screen. Composes:
   *   - task summary
   *   - optional diff preview
   *   - KV metadata (tool, model, branch, cwd, tmux, elapsed, tokens, cost, ctx)
   *   - watchdog excerpt scoped to this session
   */
  interface Props {
    s: Session;
    feed?: WatchdogEvent[];
    /** Optional diff hunks; rail shows only if provided. */
    diff?: DiffHunk;
  }
  let { s, feed = [], diff }: Props = $props();

  const state = $derived(deriveState(s));
  const scopedFeed = $derived(feed.filter(ev => !ev.ses || ev.ses === s.id || ev.ses === s.name));
</script>

<aside class="rail">
  <div class="rh">
    <span>session</span>
    <span style="color: var(--fg-3);">·</span>
    <span style="color: var(--fg);">{s.name}</span>
    <span class="spacer"></span>
    <span class="pill {state}" style="font-size: 10px;">{state}</span>
  </div>
  <div class="rb">
    {#if s.last_log || s.workdir}
      <div class="group">
        <div class="gh"><span>Task</span></div>
        <div class="task">{s.last_log ?? s.workdir}</div>
      </div>
    {/if}

    {#if diff}
      <div class="group">
        <div class="gh"><span>Diff preview</span><span style="color: var(--fg-2);">+{diff.added} -{diff.deleted}</span></div>
        <DiffBlock path={diff.path} added={diff.added} deleted={diff.deleted} lines={diff.lines} />
      </div>
    {/if}

    <div class="group">
      <div class="gh"><span>Pane</span></div>
      <div class="kv">
        <span class="k">tool</span>     <span class="v muted">{toolShort(s.tool)}</span>
        <span class="k">model</span>    <span class="v">{s.model ?? '—'}</span>
        <span class="k">cwd</span>      <span class="v">{s.workdir}</span>
        {#if s.tmux_target}
          <span class="k">tmux</span>   <span class="v">{s.tmux_target}</span>
        {/if}
        <span class="k">elapsed</span>  <span class="v">{fmtUptime(s.uptime_seconds, s.created_at)}</span>
        <span class="k">tokens</span>   <span class="v">{fmtTokens(s.tokens)}</span>
        <span class="k">cost</span>     <span class="v"><span class="acc">{fmtCost(s.cost)}</span></span>
        <span class="k">ctx</span>      <span class="v">{ctxOf(s)}%</span>
      </div>
    </div>

    {#if scopedFeed.length > 0}
      <div class="group">
        <div class="gh"><span>Watchdog · last {Math.min(scopedFeed.length, 4)}</span></div>
        <Watchdog feed={scopedFeed} limit={4} />
      </div>
    {/if}
  </div>
</aside>

<style>
  .task {
    font-size: 13px;
    color: var(--fg);
    line-height: 1.5;
    word-break: break-word;
  }
</style>
