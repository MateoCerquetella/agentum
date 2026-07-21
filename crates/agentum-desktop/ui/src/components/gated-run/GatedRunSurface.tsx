// Spec 023 Part A (AC 1–3): the gate between the "Gated run starting…" panel
// and the plain WorkspaceAgentLauncher for a surface-less workspace. Mounted
// by Terminal.tsx EXACTLY where the launcher used to render (inside the
// `absolute inset-0 z-20` overlay), so the only behavioral change is that an
// owned, booting gated run shows the starting state instead of the picker.
//
// Slice lifecycle: `maybeOfferWorkspaceHarnessRun`'s gated arm writes the
// pending slice at create; here we pin the engine-owned `current_session` into
// a terminal tab for this worktree, then clear the slice so that live session
// takes over. A halted run clears to the picker + the already-fired error
// toast. A bounded 30 s no-snapshot guard prevents a run that never registers
// from stranding the workspace on "starting".
import React, { useEffect } from 'react'
import { useAppStore } from '@/store'
import WorkspaceAgentLauncher from '../WorkspaceAgentLauncher'
import { deriveGatedRunSurface } from '@/lib/harness-run'
import { useWorktreeHarnessRun } from '@/hooks/useWorktreeHarnessRun'
import { GatedRunStartingPanel } from './GatedRunStartingPanel'

/** If no engine run matches within this window the pending slice is cleared —
 *  the workspace falls back to the picker rather than claiming "starting"
 *  forever (a failed start-work already toasted at the composer). */
const NO_RUN_GRACE_MS = 30_000

export default function GatedRunSurface({
  worktreeId
}: {
  worktreeId: string
}): React.JSX.Element {
  const pending = useAppStore((s) => s.gatedRunStartingByWorktreeId[worktreeId] !== undefined)
  const clearStarting = useAppStore((s) => s.clearGatedRunStarting)
  const workdir = useAppStore((s) => {
    const worktree = Object.values(s.worktreesByRepo ?? {})
      .flat()
      .find((w) => w.id === worktreeId)
    return worktree?.path
  })
  const { run } = useWorktreeHarnessRun(workdir)

  // The overlay only mounts when the worktree has NO surface, so an
  // attachable session reads as `hasNoSurface === false` upstream — here the
  // engine session's own presence is the attach signal.
  const surface = deriveGatedRunSurface({
    pendingGatedRun: pending,
    harness: run,
    hasAttachableSession: false
  })

  // The architect deliberately kept harness sessions server-owned (no
  // `worktree_path` stamp), so merely observing `current_session` does not make
  // a normal worktree tab appear. Reuse the existing pinned-session tab path
  // (also used by the tmux session viewer): it streams the exact live server
  // session while close/unmount only detaches the view — the harness retains
  // lifecycle ownership. This is the missing AC-2 bridge from by-workdir run
  // discovery to an actually visible running agent.
  useEffect(() => {
    const sessionId = run?.current_session
    if (!pending || !sessionId) return

    const state = useAppStore.getState()
    const existing = (state.tabsByWorktree[worktreeId] ?? []).find(
      (tab) => tab.serverSessionId === sessionId
    )
    if (existing) {
      state.setActiveTab(existing.id)
    } else {
      const tab = state.createTab(worktreeId, undefined, undefined, {
        activate: true,
        recordInteraction: false,
        persistTmux: true,
        serverSessionId: sessionId
      })
      state.setTabCustomTitle(tab.id, 'Gated run')
    }
    state.clearGatedRunStarting(worktreeId)
  }, [pending, run?.current_session, worktreeId])

  // Clear-on-transition (AC 1's "clears once the engine-spawned session is
  // attachable"): 'session' → the tab takes over; 'picker' with a halted run
  // → nothing is starting anymore.
  useEffect(() => {
    if (pending && surface !== 'starting') {
      clearStarting(worktreeId)
    }
  }, [pending, surface, clearStarting, worktreeId])

  // Bounded guard: no run snapshot within the grace window → never strand
  // the workspace on "starting".
  useEffect(() => {
    if (!pending || run) return
    const timer = setTimeout(() => clearStarting(worktreeId), NO_RUN_GRACE_MS)
    return () => clearTimeout(timer)
  }, [pending, run, clearStarting, worktreeId])

  if (pending && surface === 'starting') {
    const featureLabel =
      run?.features.features.find((f) => f.id === run.current_feature)?.name ?? null
    return (
      <GatedRunStartingPanel
        stateLabel={run?.state ?? null}
        phaseLabel={run?.phase ?? null}
        featureLabel={featureLabel}
      />
    )
  }
  return <WorkspaceAgentLauncher worktreeId={worktreeId} />
}
