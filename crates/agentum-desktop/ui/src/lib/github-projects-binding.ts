// Pure state logic for the Projects v2 board-binding editor (spec 010 F1).
// No DOM, no IPC — vitest'able without jsdom (the UI package ships none).
// The wire shapes mirror `crates/agentum-server/src/routes/github_projects.rs`.

export type BoardPhaseKey = 'todo' | 'inProgress' | 'readyToTest' | 'done' | 'blocked'

/** Canonical render order for the five per-phase selects. */
export const BOARD_PHASES: readonly BoardPhaseKey[] = [
  'todo',
  'inProgress',
  'readyToTest',
  'done',
  'blocked'
]

export const BOARD_PHASE_LABELS: Record<BoardPhaseKey, string> = {
  todo: 'Todo',
  inProgress: 'In Progress',
  readyToTest: 'Ready to Test',
  done: 'Done',
  blocked: 'Blocked'
}

export type DiscoveredStatusOption = { id: string; name: string }

export type ResolvedPhaseDto = {
  optionId: string
  name: string
  /** `fell_back` ⇒ the phase had no synonym match and rides the In Progress
   *  option — render the D5 hint chip. */
  via: 'matched' | 'fell_back'
}

export type ResolvedMappingDto = Record<BoardPhaseKey, ResolvedPhaseDto>

/** Option-id per phase; `''` = not chosen yet (Save stays disabled). */
export type BindingSelection = Record<BoardPhaseKey, string>

export const EMPTY_SELECTION: BindingSelection = {
  todo: '',
  inProgress: '',
  readyToTest: '',
  done: '',
  blocked: ''
}

export type SelectionAction =
  | { type: 'set'; phase: BoardPhaseKey; optionId: string }
  | { type: 'reset'; selection: BindingSelection }

/** The select-state reducer: one phase changes, or the whole selection resets
 *  (a fresh discovery / loading a stored binding). */
export function reduceBindingSelection(
  state: BindingSelection,
  action: SelectionAction
): BindingSelection {
  switch (action.type) {
    case 'set':
      return { ...state, [action.phase]: action.optionId }
    case 'reset':
      return { ...action.selection }
  }
}

/** Pre-select every phase from the server's resolved mapping; a refusal
 *  (`resolved: null`) yields empty selects — the manual-completion prompt,
 *  never a partial pre-selection. */
export function selectionFromResolved(resolved: ResolvedMappingDto | null): BindingSelection {
  if (!resolved) {
    return { ...EMPTY_SELECTION }
  }
  return {
    todo: resolved.todo.optionId,
    inProgress: resolved.inProgress.optionId,
    readyToTest: resolved.readyToTest.optionId,
    done: resolved.done.optionId,
    blocked: resolved.blocked.optionId
  }
}

/**
 * Pre-selection when RE-discovering an already-bound repo: prefer each stored
 * option id that still exists on the field (edits survive a re-discover),
 * fall back to the fresh `resolved` value (a deleted column heals), else ''.
 */
export function selectionForRebind(
  stored: Record<BoardPhaseKey, string>,
  resolved: ResolvedMappingDto | null,
  options: DiscoveredStatusOption[]
): BindingSelection {
  const live = new Set(options.map((o) => o.id))
  const fromResolved = selectionFromResolved(resolved)
  const pick = (phase: BoardPhaseKey): string =>
    live.has(stored[phase]) ? stored[phase] : fromResolved[phase]
  return {
    todo: pick('todo'),
    inProgress: pick('inProgress'),
    readyToTest: pick('readyToTest'),
    done: pick('done'),
    blocked: pick('blocked')
  }
}

/** The Save gate: every phase has an option id (AC 1's constructor invariant,
 *  enforced in the UI before the PUT's own 400 gate). */
export function mappingComplete(selection: BindingSelection): boolean {
  return BOARD_PHASES.every((phase) => selection[phase].trim().length > 0)
}

/**
 * D5: fallback hints for FellBack phases — visible, never silent. Keyed by
 * phase; the text names the missing column and the recovery (add + re-discover).
 */
export function fallbackHints(
  resolved: ResolvedMappingDto | null
): Partial<Record<BoardPhaseKey, string>> {
  if (!resolved) {
    return {}
  }
  const hints: Partial<Record<BoardPhaseKey, string>> = {}
  for (const phase of BOARD_PHASES) {
    if (resolved[phase].via === 'fell_back') {
      hints[phase] =
        `No "${BOARD_PHASE_LABELS[phase]}"-like column — falls back to ` +
        `"${resolved[phase].name}". Add one on GitHub and re-discover to map it.`
    }
  }
  return hints
}

/** Option names for the selected ids (the binding's display metadata, sent on
 *  PUT so the settings surface can label a stored mapping without a fetch). */
export function optionNamesForSelection(
  selection: BindingSelection,
  options: DiscoveredStatusOption[]
): Record<BoardPhaseKey, string> {
  const byId = new Map(options.map((o) => [o.id, o.name]))
  return {
    todo: byId.get(selection.todo) ?? '',
    inProgress: byId.get(selection.inProgress) ?? '',
    readyToTest: byId.get(selection.readyToTest) ?? '',
    done: byId.get(selection.done) ?? '',
    blocked: byId.get(selection.blocked) ?? ''
  }
}
