// Pure derivation model for the spec-014 tracker-phase chip: `/api/events`
// frame → tracker event, event → worktree join, (persisted phase + live
// overlay) → chip state. Kept IO-free (no store, no socket) so it's trivially
// testable — mirrors lib/server-worktree-activity-map.ts.

/** The tracker pipeline phase wire form (task_sink::TrackerPhase::wire_str). */
export type TrackerPhaseWire = 'todo' | 'in_progress' | 'in_review' | 'ready_to_test' | 'done'

const PHASE_WIRE_VALUES: readonly TrackerPhaseWire[] = [
  'todo',
  'in_progress',
  'in_review',
  'ready_to_test',
  'done'
]

/** Parse a persisted/event phase string; junk → null (never a fabricated phase). */
export function parseTrackerPhaseWire(value: unknown): TrackerPhaseWire | null {
  return PHASE_WIRE_VALUES.includes(value as TrackerPhaseWire) ? (value as TrackerPhaseWire) : null
}

/** A distilled `tracker.*` bus event. `worktreeId` is null for tracker-coord-
 *  only emitters (harness / MCP / planning) — consumers join on `trackerUrl`. */
export type TrackerLiveEvent =
  | { kind: 'phase'; worktreeId: string | null; trackerUrl: string | null; phase: TrackerPhaseWire }
  | { kind: 'blocked'; worktreeId: string | null; trackerUrl: string | null }

function payloadString(payload: unknown, key: string): string | null {
  if (!payload || typeof payload !== 'object') {
    return null
  }
  const value = (payload as Record<string, unknown>)[key]
  return typeof value === 'string' && value.length > 0 ? value : null
}

/**
 * Distill a raw `/api/events` frame into a tracker event, or null for kinds we
 * don't track / malformed payloads. A `tracker.phase_changed` with an
 * unparseable phase reads as null — the bus never makes the chip lie.
 */
export function trackerEventFromFrame(ev: {
  kind?: unknown
  payload?: unknown
}): TrackerLiveEvent | null {
  if (ev.kind === 'tracker.phase_changed') {
    const phase = parseTrackerPhaseWire(
      ev.payload && typeof ev.payload === 'object'
        ? (ev.payload as { phase?: unknown }).phase
        : undefined
    )
    if (!phase) {
      return null
    }
    return {
      kind: 'phase',
      worktreeId: payloadString(ev.payload, 'worktree_id'),
      trackerUrl: payloadString(ev.payload, 'tracker_url'),
      phase
    }
  }
  if (ev.kind === 'tracker.blocked') {
    return {
      kind: 'blocked',
      worktreeId: payloadString(ev.payload, 'worktree_id'),
      trackerUrl: payloadString(ev.payload, 'tracker_url')
    }
  }
  return null
}

/** Minimal worktree row shape the join needs. */
export type TrackerWorktreeRow = {
  id: string
  trackerUrl?: string | null
}

/**
 * Resolve which worktree a tracker event belongs to: the emitter-supplied
 * `worktree_id` when present, else a `trackerUrl` fallback (harness/MCP events
 * carry `worktree_id: null`; in the issue-first flow the workspace and its
 * features share the issue URL). No match → null — never a fabricated join.
 */
export function matchEventToWorktree(
  evt: TrackerLiveEvent,
  rows: readonly TrackerWorktreeRow[]
): string | null {
  if (evt.worktreeId) {
    const byId = rows.find((row) => row.id === evt.worktreeId)
    if (byId) {
      return byId.id
    }
  }
  if (evt.trackerUrl) {
    const byUrl = rows.find((row) => row.trackerUrl === evt.trackerUrl)
    if (byUrl) {
      return byUrl.id
    }
  }
  return null
}

/** The live overlay the store keeps per worktree (event-derived hints). */
export type TrackerLiveOverlay = {
  phase?: TrackerPhaseWire
  attention: boolean
}

/** What the chip renders. `attention` = the needs-attention (blocked) variant. */
export type TrackerChipState = {
  phase: TrackerPhaseWire | null
  label: string
  attention: boolean
}

const PHASE_LABELS: Record<TrackerPhaseWire, string> = {
  todo: 'Todo',
  in_progress: 'In Progress',
  in_review: 'In Review',
  ready_to_test: 'Ready to Test',
  done: 'Done'
}

/**
 * Derive the chip from the persisted `trackerPhase` (cold truth, AC 4) layered
 * with the live event overlay (hint — the lossy bus is never the only source).
 * Null ⇒ render nothing: an unbound worktree (no persisted phase, no live
 * event) never shows a fabricated phase (AC 6). A blocked event alone (no
 * phase known yet) still surfaces attention — the payload-only contract of
 * AC 11.
 */
export function deriveTrackerChip(
  persistedPhase: string | null | undefined,
  live: TrackerLiveOverlay | undefined
): TrackerChipState | null {
  const phase = live?.phase ?? parseTrackerPhaseWire(persistedPhase)
  const attention = live?.attention ?? false
  if (!phase && !attention) {
    return null
  }
  return {
    phase,
    label: phase ? PHASE_LABELS[phase] : 'Blocked',
    attention
  }
}
