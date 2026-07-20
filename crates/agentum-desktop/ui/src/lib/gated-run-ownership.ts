// Pure decision behind the composer's "Start gated run" fallback. When a gated
// run is armed the plain agent delivery is suppressed so exactly one
// (engine-spawned) agent drives the worktree — but that suppression is only
// safe when the engine ACTUALLY took ownership. If it didn't (start-work
// failed, the issue was ineligible, or the plan produced zero features) the
// worktree would strand on the empty "Start a session" picker with nothing
// driving it. This module answers "did the engine take ownership?" so the caller
// can fall back to a normal session instead of a silent empty worktree.

/** The relevant fields of a `POST /api/harness/start-work` success response. */
export type GatedRunStartResult = {
  /** Features the plan produced — a fresh run with zero has nothing to drive. */
  planned: number
  /** A live run already owns this worktree (left untouched). */
  alreadyRunning: boolean
}

/**
 * Whether a *successful* start-work response means the engine owns the worktree.
 * An already-live run owns it regardless of this call's plan count; a fresh run
 * owns it only when the plan produced at least one feature to drive. A thrown
 * request or an ineligible issue never reaches here — those are `false` at the
 * call site (no engine, so always fall back to a normal session).
 */
export function gatedRunResultOwnsWorktree(result: GatedRunStartResult): boolean {
  return result.alreadyRunning || result.planned > 0
}
