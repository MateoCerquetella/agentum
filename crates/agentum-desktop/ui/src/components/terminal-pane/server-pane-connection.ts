// Additive, opt-in Option A path: connect a terminal pane to a server session
// (a tmux pane streamed over WS by the embedded agentum-server) instead of a
// local PTY. Mirrors connectPanePty's PanePtyBinding contract so the pane
// lifecycle treats both identically. Off by default — see shouldUseServerTerminals.
import type { PaneManager, ManagedPane } from '@/lib/pane-manager/pane-manager'
import {
  connectPanePty,
  shouldWritePtyOutputForeground,
  type PanePtyBinding
} from './pty-connection'
import { getSession } from '@/runtime/agentum-server-client'
import type { PtyConnectionDeps } from './pty-connection-types'
import { useAppStore } from '@/store'
import { ensureWorkspaceSession, sessionName } from '@/runtime/workspace-session'
import { resolveServerHostIdForConnection } from '@/runtime/server-host-client'
import { getRepoMapFromState, getWorktreeMapFromState } from '@/store/selectors'
import {
  bindServerSessionTerminal,
  type ServerSessionTerminalBinding
} from '@/runtime/server-session-terminal'
import { detectAgentStatusFromTitle } from '@/lib/agent-status'
import { makePaneKey } from '../../../../shared/stable-pane-id'
import { createPaneActivityTracker, type PaneActivityTracker } from './pane-activity-tracker'
import type { AgentType } from '../../../../shared/agent-status-types'
import { registerServerSessionActivity } from '@/runtime/server-session-activity'

/** The tab's launch agent (claude/codex/…) drives the server session's tool;
 *  a plain terminal tab has none, so it runs a shell. */
function resolveSessionTool(deps: PtyConnectionDeps): string {
  const tab = useAppStore
    .getState()
    .tabsByWorktree[deps.worktreeId]?.find((t) => t.id === deps.tabId)
  return tab?.launchAgent ?? 'terminal'
}

/** The tab's pinned server session id, when it was opened by attaching to a
 *  discovered external tmux session. Such panes must stream exactly that
 *  session — never the workdir-keyed find-or-create. */
function resolvePinnedSessionId(deps: PtyConnectionDeps): string | undefined {
  return useAppStore
    .getState()
    .tabsByWorktree[deps.worktreeId]?.find((t) => t.id === deps.tabId)?.serverSessionId
}

/**
 * Default ON — terminals run as real tmux sessions in the embedded
 * agentum-server (the local Tauri PTY path is a half-ported stub). Set
 * `localStorage['agentum.serverTerminals'] = '0'` to force the local path.
 */
export function shouldUseServerTerminals(): boolean {
  try {
    return globalThis.localStorage?.getItem('agentum.serverTerminals') !== '0'
  } catch {
    return true
  }
}

/**
 * Drop-in alternative to `connectPanePty`: ensure a server session exists for
 * this pane's workspace (workdir) and bind its tmux pane to the xterm. If the
 * server path can't establish (no workdir, session/stream failure), it falls
 * back to `connectPanePty` so the pane still works — the native PTY path,
 * which preserves the repo's local/SSH host boundary.
 */
export function connectPaneServerSession(
  pane: ManagedPane,
  manager: PaneManager,
  deps: PtyConnectionDeps
): PanePtyBinding {
  let disposed = false
  let binding: ServerSessionTerminalBinding | null = null
  // When the server path fails, we hand the pane to connectPanePty and delegate
  // every binding method to it — the lifecycle hook never knows the difference.
  let localFallback: PanePtyBinding | null = null
  // Unregisters this pane from the server-side agent-activity event stream (the
  // watchdog's awaiting-input / working / finished verdicts). Set after the
  // server session binds; called in dispose.
  let unregisterActivity: (() => void) | null = null
  // Synthetic pty id so the sidebar's title-derived agent rows treat this pane
  // as a live PTY (buildTitleDerivedAgentRows gates on tabHasLivePty). Cleared
  // on dispose. `server:` prefix keeps it distinct from real local pty ids.
  let registeredPtyId: string | null = null
  const paneKey = makePaneKey(deps.tabId, pane.leafId)
  // A tab launched with an agent keeps a live agent session even when its idle
  // title is unrecognizable (codex's idle title is just the cwd basename), so
  // treat an unknown title as idle rather than dropping the status entirely.
  const sessionTool = resolveSessionTool(deps)
  const isAgentTab = sessionTool !== 'terminal'

  // ── Byte-flow activity fallback ──────────────────────────────────────────
  // Agents whose OSC title never reports working (OpenCode, Codex) would show
  // a permanent "idle" sidebar dot even mid-turn, because the title path has
  // nothing to classify. We watch the pane's byte stream instead — the same
  // "pane is redrawing" signal the daemon watchdog polls for — and write an
  // explicit working/idle entry into agentStatusByPaneKey (which the sidebar
  // renders ahead of title-derived rows). The moment a title DOES report
  // working/permission for this pane, `titleCarriesState` latches true and we
  // hand authority back to the precise title path (Claude/Cursor/Gemini), so
  // those agents are never double-driven.
  const ACTIVITY_IDLE_AFTER_MS = 3000
  // Suppress the burst tmux replays on attach (RIS + one capture-pane
  // snapshot) so a pane that is actually idle when we connect doesn't flicker
  // working for one idle window.
  const ACTIVITY_GRACE_MS = 1500
  // Re-stamp a long-running working entry before the 30-min staleness decay
  // would silently flip it to idle while the agent is still streaming.
  const ACTIVITY_REFRESH_MS = 60_000
  const bindStartedAt = Date.now()
  let titleCarriesState = false
  let lastKnownTitle: string | null = null
  let activityTracker: PaneActivityTracker | null = null
  let lastWorkingSetAt = 0

  const setByteWorkingStatus = (): void => {
    const store = useAppStore.getState()
    store.clearServerAgentDone(paneKey)
    store.setAgentStatus(paneKey, {
      state: 'working',
      prompt: '',
      agentType: sessionTool as AgentType
    })
    lastWorkingSetAt = Date.now()
  }

  const ensureActivityTracker = (): PaneActivityTracker => {
    if (!activityTracker) {
      activityTracker = createPaneActivityTracker({
        idleAfterMs: ACTIVITY_IDLE_AFTER_MS,
        onWorking: () => {
          if (disposed) {
            return
          }
          setByteWorkingStatus()
        },
        onIdle: () => {
          if (disposed) {
            return
          }
          const store = useAppStore.getState()
          // Drop the working override so the title-derived idle row surfaces,
          // then mark the turn done (green ✓) — mirrors the title path's
          // working→idle completion for title-signaling agents.
          store.removeAgentStatus(paneKey)
          store.markServerAgentDone(paneKey)
          if (agentTaskCompleteNotificationsEnabled()) {
            deps.dispatchNotification({
              source: 'agent-task-complete',
              terminalTitle: lastKnownTitle ?? sessionTool,
              paneKey
            })
          }
        }
      })
    }
    return activityTracker
  }

  const disposeActivityTracker = (): void => {
    activityTracker?.dispose()
    activityTracker = null
  }

  // Hand authority to the precise title path the first time a title reports a
  // real working/permission state for this pane; tear down any byte-derived
  // override so the two never fight.
  const yieldToTitleAuthority = (): void => {
    if (titleCarriesState) {
      return
    }
    titleCarriesState = true
    disposeActivityTracker()
    useAppStore.getState().removeAgentStatus(paneKey)
  }

  const handleServerSessionActivity = (): void => {
    if (disposed || !isAgentTab || titleCarriesState) {
      return
    }
    if (Date.now() - bindStartedAt < ACTIVITY_GRACE_MS) {
      return
    }
    const tracker = ensureActivityTracker()
    tracker.noteActivity()
    // Keep a long continuous burst fresh so it doesn't decay to idle mid-turn.
    if (Date.now() - lastWorkingSetAt >= ACTIVITY_REFRESH_MS) {
      setByteWorkingStatus()
    }
  }
  // Last COMMITTED status — drives the working→idle completion notification and
  // the spinner-flicker debounce below.
  let committedTitleStatus: 'working' | 'permission' | 'idle' | null = null
  let idleHoldTimer: ReturnType<typeof setTimeout> | null = null
  let pendingIdleTitle: string | null = null
  // Why: codex animates its spinner by interleaving a bare cwd title between
  // braille frames mid-turn. A working→idle edge must persist for this window
  // before it counts as a real turn end — otherwise every flicker fires a false
  // completion notification and blinks the sidebar dot. Mirrors the local PTY
  // path's WORKING_TITLE_HOLD_MS.
  const WORKING_TO_IDLE_HOLD_MS = 700

  const clearIdleHold = (): void => {
    if (idleHoldTimer) {
      clearTimeout(idleHoldTimer)
      idleHoldTimer = null
    }
    pendingIdleTitle = null
  }

  const agentTaskCompleteNotificationsEnabled = (): boolean => {
    const notifications = useAppStore.getState().settings?.notifications
    return notifications?.enabled !== false && notifications?.agentTaskComplete !== false
  }

  const commitServerSessionStatus = (
    title: string,
    status: 'working' | 'permission' | 'idle'
  ): void => {
    deps.setRuntimePaneTitle(deps.tabId, pane.id, title)
    if (manager.getActivePane()?.id === pane.id) {
      deps.updateTabTitle(deps.tabId, title)
    }
    const justFinished = committedTitleStatus === 'working' && status === 'idle'
    if (justFinished && agentTaskCompleteNotificationsEnabled()) {
      deps.dispatchNotification({ source: 'agent-task-complete', terminalTitle: title, paneKey })
    }
    // Green ✓ "done" on a real turn end; cleared the moment the agent works
    // again (or is torn down, in dispose). A fresh idle that never worked stays
    // grey because justFinished is only true on a working→idle edge.
    const store = useAppStore.getState()
    if (justFinished) {
      store.markServerAgentDone(paneKey)
    } else if (status === 'working' || status === 'permission') {
      store.clearServerAgentDone(paneKey)
    }
    committedTitleStatus = status
  }

  // Why: server-session bytes go straight to xterm and never touched the
  // agent-status pipeline, so the sidebar dot stayed blank and no "task
  // complete" notification ever fired for tmux-backed agents. Route each OSC
  // title into runtimePaneTitlesByTabId (what buildTitleDerivedAgentRows reads),
  // map a known agent's unrecognized title to idle (so the row survives a turn
  // end), and raise the completion notification on a SUSTAINED working→idle.
  const handleServerSessionTitle = (title: string): void => {
    if (disposed) {
      return
    }
    // Parity with the local path: cursor-agent's bare native title carries no
    // status and must not stomp a live working/idle state back to nothing.
    if (title.trim().toLowerCase() === 'cursor agent') {
      return
    }
    lastKnownTitle = title
    const detected = detectAgentStatusFromTitle(title)
    // This pane's title actually signals activity — let the precise title path
    // own it and disable the byte-flow fallback.
    if (detected === 'working' || detected === 'permission') {
      yieldToTitleAuthority()
    }
    const status: 'working' | 'permission' | 'idle' | null =
      detected ?? (isAgentTab ? 'idle' : null)
    if (status === null) {
      // Plain shell line on a non-agent tab — reflect it with no status meaning.
      deps.setRuntimePaneTitle(deps.tabId, pane.id, title)
      if (manager.getActivePane()?.id === pane.id) {
        deps.updateTabTitle(deps.tabId, title)
      }
      return
    }
    if (status === 'working' || status === 'permission') {
      // A live frame wins immediately and cancels a pending completion, so
      // codex's bare-title flicker can never read as a finished turn.
      clearIdleHold()
      commitServerSessionStatus(title, status)
      return
    }
    // status === 'idle': hold a working→idle edge briefly; a returning working
    // frame cancels it. Only a sustained idle commits and notifies.
    if (committedTitleStatus === 'working') {
      pendingIdleTitle = title
      if (!idleHoldTimer) {
        idleHoldTimer = setTimeout(() => {
          idleHoldTimer = null
          const heldTitle = pendingIdleTitle ?? title
          pendingIdleTitle = null
          if (!disposed) {
            commitServerSessionStatus(heldTitle, 'idle')
          }
        }, WORKING_TO_IDLE_HOLD_MS)
      }
      return
    }
    commitServerSessionStatus(title, status)
  }

  const fallBackToLocal = (reason: string): void => {
    if (disposed || localFallback) {
      return
    }
    console.warn(`[agentum] server terminal unavailable, using native PTY: ${reason}`)
    localFallback = connectPanePty(pane, manager, deps)
  }

  const workdir = deps.cwd ?? ''
  const pinnedSessionId = resolvePinnedSessionId(deps)
  if (!workdir && !pinnedSessionId) {
    fallBackToLocal('no workdir')
  } else {
    const tool = resolveSessionTool(deps)
    // The desktop launches agents by typing a command into a shell (e.g.
    // `claude`). For a shell session, forward that startup command so the agent
    // actually attaches. For an agent-tool session the server launches it, so
    // sending the command again would double-launch — skip it. A pinned
    // external session already runs whatever the user left in it; never inject.
    // This is only a CANDIDATE: an `onlyIfFresh` startup (the worktree-reopen
    // agent relaunch) is additionally gated on `freshPane` below, so a reattach
    // to a surviving tmux pane (agent likely still running in it) never gets
    // the command typed into the agent's composer.
    const startup = tool === 'terminal' && !pinnedSessionId ? deps.startup : undefined
    void (async () => {
      try {
        // Why: a remote (SSH) worktree's repo carries a `connectionId` (native
        // SSH target). Resolve it to a server host id so the session's tmux pane
        // runs ON THE REMOTE over SSH — the same path the TUI uses. Local repos
        // have no connectionId and run on the local host (hostId undefined).
        const state = useAppStore.getState()
        const worktree = getWorktreeMapFromState(state).get(deps.worktreeId)
        const repo = worktree ? getRepoMapFromState(state).get(worktree.repoId) : null
        const connectionId = repo?.connectionId ?? null
        const hostId = connectionId
          ? await resolveServerHostIdForConnection(connectionId)
          : undefined
        if (disposed) {
          return
        }
        // A pinned tab streams exactly its (externally-attached) session; the
        // workdir find-or-create would bind a different pane. If the pinned
        // session record is gone (deleted server-side), surface the error —
        // falling back to a fresh local shell here would silently masquerade
        // as the remote session.
        // Per-tab name for AGENT tabs so each launch gets its own tmux pane:
        // clicking Cursor (or OpenCode) three times in one project yields three
        // independent sessions instead of reattaching to one shared pane. The
        // name is keyed by this tab's id, so the SAME tab remounting/reconnecting
        // reattaches to its own pane (idempotent) while a NEW tab spawns a fresh
        // one. Plain `terminal` tabs pass no name and keep the shared workspace
        // session (the git/fs surface depends on that single pane).
        const sessionNameForTab =
          tool === 'terminal' ? undefined : sessionName(workdir, tool, deps.tabId)
        const session = pinnedSessionId
          ? await getSession(pinnedSessionId)
          : await ensureWorkspaceSession({ workdir, tool, hostId, name: sessionNameForTab })
        if (disposed) {
          return
        }
        // Only type an `onlyIfFresh` launch command into a FRESHLY spawned
        // pane (bare shell). Reattached panes still run whatever was in them —
        // typing `claude` again would submit the word as a prompt to the
        // already running agent. Explicit user commands (quick commands) have
        // no `onlyIfFresh` and always run.
        const freshPane = 'freshPane' in session && session.freshPane === true
        const startupCommand =
          startup && (!startup.onlyIfFresh || freshPane) ? startup.command : undefined
        binding = await bindServerSessionTerminal(session.id, pane.terminal, {
          startupCommand,
          onTitle: handleServerSessionTitle,
          onActivity: handleServerSessionActivity,
          // Live per-write visibility, read from the same ref the native PTY
          // path uses, so the scheduler throttles this pane's output only while
          // it is a background pane and writes synchronously while it is focused.
          isForeground: () => shouldWritePtyOutputForeground(deps.isVisibleRef.current),
          // Bucket this pane's WS throughput by host so the status-bar I/O chip
          // can show per-host rates. Mirrors the hosts-slice HostKey scheme:
          // `local` for the daemon's machine, `ssh:<connectionId>` for a remote.
          hostKey: connectionId ? `ssh:${connectionId}` : 'local'
        })
        if (disposed) {
          binding.dispose()
          binding = null
          return
        }
        // Mark the tab as having a live PTY so title-derived agent rows render.
        registeredPtyId = `server:${session.id}:${pane.leafId}`
        deps.updateTabPtyId(deps.tabId, registeredPtyId)
        // Why: TRUTHFUL tmux signal for the tab bar. Record this pane as
        // tmux-backed ONLY when the session is genuinely running in a real tmux
        // session (`tmux_target` non-null) — the same truth the host-header glyph
        // uses. The local-PTY fallback path never reaches here, so PTY tabs stay
        // icon-less. (Never derive this from persistTmux — that intent flag lied
        // about local PTYs and is explicitly not used.)
        if (session.status === 'running' && session.tmux_target) {
          useAppStore.getState().markPaneTmux(paneKey)
        }

        // ── Watchdog agent-activity → sidebar dot ───────────────────────────
        // Subscribe to the server's authoritative activity verdicts for this
        // session. The title/byte path above can't see "agent paused to ask
        // you something" (it looks like a working→idle edge), and starts cold
        // after a reload; the watchdog knows both. On connect the server
        // replays the current state per session, so this also seeds the dot
        // for an agent that was already running/blocked before this pane
        // mounted.
        unregisterActivity = registerServerSessionActivity(session.id, {
          onAwaitingInput: () => {
            if (disposed) {
              return
            }
            const store = useAppStore.getState()
            // Amber "needs attention" outranks the green ✓ the title/byte path
            // may have just set on the working→idle edge.
            store.markAwaitingInput(paneKey)
            store.clearServerAgentDone(paneKey)
          },
          onInputResolved: (state) => {
            if (disposed) {
              return
            }
            const store = useAppStore.getState()
            store.clearAwaitingInput(paneKey)
            if (state === 'working' && !titleCarriesState) {
              setByteWorkingStatus()
            } else if (state !== 'working') {
              // Prompt dismissed → idle: drop any byte/seed working override so
              // the dot settles instead of spinning.
              store.removeAgentStatus(paneKey)
            }
          },
          onWorking: () => {
            if (disposed) {
              return
            }
            useAppStore.getState().clearAwaitingInput(paneKey)
            // Seed the working dot only when the precise title path hasn't taken
            // authority — for title-signaling agents (Claude/Cursor/Gemini) that
            // path owns working/done and must not be double-driven.
            if (!titleCarriesState) {
              setByteWorkingStatus()
            }
          },
          onFinished: () => {
            if (disposed) {
              return
            }
            const store = useAppStore.getState()
            store.clearAwaitingInput(paneKey)
            // Clear a byte/seed working override so a finished agent doesn't keep
            // spinning. The title path owns the green ✓ for title-signaling agents.
            if (!titleCarriesState) {
              store.removeAgentStatus(paneKey)
            }
          }
        })
      } catch (error) {
        if (!disposed) {
          if (pinnedSessionId) {
            // No local fallback for an external attach — a fresh local shell
            // would silently impersonate the remote session.
            pane.terminal.write(
              `\r\n\x1b[2m[agentum: could not attach to remote tmux session — ${String(error)}]\x1b[0m\r\n`
            )
          } else {
            fallBackToLocal(String(error))
          }
        }
      }
    })()
  }

  return {
    dispose: () => {
      disposed = true
      clearIdleHold()
      disposeActivityTracker()
      // Stop receiving watchdog activity for this (now gone) pane and drop any
      // amber "needs attention" marker so a closed pane never lingers as blocked.
      unregisterActivity?.()
      unregisterActivity = null
      useAppStore.getState().clearAwaitingInput(paneKey)
      // Clear any byte-derived working override so a torn-down pane doesn't
      // leave a stuck "working" row in the sidebar.
      useAppStore.getState().removeAgentStatus(paneKey)
      useAppStore.getState().clearServerAgentDone(paneKey)
      // Pane is gone — drop its tmux marker so the tab's glyph clears the moment
      // the session detaches (kept in lockstep with the done marker above).
      useAppStore.getState().clearPaneTmux(paneKey)
      if (registeredPtyId) {
        deps.clearTabPtyId(deps.tabId, registeredPtyId)
        deps.clearRuntimePaneTitle(deps.tabId, pane.id)
        registeredPtyId = null
      }
      binding?.dispose()
      binding = null
      localFallback?.dispose()
      localFallback = null
    },
    // Delegate to the local binding when we fell back; no-ops for the server
    // path (the server owns pane lifecycle / process tracking).
    syncRendererOutputVisibility: () => localFallback?.syncRendererOutputVisibility(),
    syncProcessTracking: () => localFallback?.syncProcessTracking(),
    // Force a clean repaint: the server binding nudges the agent (SIGWINCH +
    // fresh snapshot) to heal a corrupted grid. When we fell back to a local
    // PTY there's no server pane to nudge — delegate if that path ever grows
    // the capability, else it's a no-op.
    forceRedraw: () => (binding ? binding.forceRedraw() : localFallback?.forceRedraw?.()),
    // Route intercepted keyboard chords (word-nav/erase, line-nav) into the
    // session's input stream. The local-fallback pane sends chords via its
    // registered paneTransport, so keyboard-handlers reaches it there and never
    // calls this — but delegate anyway for symmetry.
    sendChordInput: (data: string) =>
      binding ? binding.sendInput(data) : localFallback?.sendChordInput?.(data)
  }
}
