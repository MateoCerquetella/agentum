// Spec 008 F2 — the Chat "Complex feature" staged Socratic interview is driven
// CLIENT-side (D1/D-B): the server is a pure `(mode, stage) → prompt` function
// and owns NO stage state, so the "advances exactly one pass per user turn and
// never skips" progression (AC 7) is a CLIENT invariant. This module is the pure
// state machine for that mode — no React, no DOM, no xterm — so the invariant is
// unit-testable in isolation (the AC-7 "unit-tested progression" pin).

/** Which intake experience a Chat message uses. Mirrors the server's
 *  `IntakeMode` (`routes/chat.rs`); the wire values are the snake_case strings. */
export type IntakeMode = 'fast' | 'socratic'

/** The five Socratic passes (spec 008 F2): WHO → WHAT → WHY → done-criteria →
 *  risks. `stage` travels in the request; the server maps it to a per-pass
 *  prompt. */
export const SOCRATIC_FIRST_STAGE = 1
export const SOCRATIC_FINAL_STAGE = 5

/** The per-conversation intake state (D1: rides the existing localStorage chat
 *  history, no new store table). `mode` is the experience; `stage` is the
 *  socratic pass the NEXT user turn will run (meaningful only for socratic).
 *  Absent on a conversation ⇒ Fast (back-compat with pre-008 chats). */
export type IntakeState = { mode: IntakeMode; stage: number }

/** Clamp any stage into the valid 1..=5 pass range (defensive — the server also
 *  clamps). Non-finite / fractional values coerce sanely so a corrupt
 *  localStorage value can never strand the interview off-range. */
export function clampStage(stage: number): number {
  if (!Number.isFinite(stage)) return SOCRATIC_FIRST_STAGE
  const n = Math.floor(stage)
  if (n < SOCRATIC_FIRST_STAGE) return SOCRATIC_FIRST_STAGE
  if (n > SOCRATIC_FINAL_STAGE) return SOCRATIC_FINAL_STAGE
  return n
}

/** The intake a Fast message is sent with. */
export function fastIntake(): IntakeState {
  return { mode: 'fast', stage: SOCRATIC_FIRST_STAGE }
}

/** The intake the FIRST Complex message is sent with (pass 1 — WHO). */
export function socraticIntake(): IntakeState {
  return { mode: 'socratic', stage: SOCRATIC_FIRST_STAGE }
}

/** Advance the Socratic interview by exactly ONE pass on a user turn — the AC-7
 *  progression invariant ("advances exactly one pass per user turn and never
 *  skips", capped at the final pass 5). Fast never advances. Pure:
 *  `(state) → next state`.
 *
 *  Called after a user turn is sent at `state.stage` to compute the stage the
 *  NEXT turn will use. At the final pass it stays at 5 (the composer then offers
 *  the same "Preview issues" convergence as Fast rather than skipping a pass). */
export function advanceIntake(state: IntakeState): IntakeState {
  if (state.mode !== 'socratic') return state
  return { mode: 'socratic', stage: clampStage(state.stage + 1) }
}

/** Whether the Socratic interview has reached its final pass — the point the
 *  composer stops advancing and leans on "Preview issues" (the convergence Fast
 *  shares). Always false for Fast. */
export function isSocraticComplete(state: IntakeState): boolean {
  return state.mode === 'socratic' && clampStage(state.stage) >= SOCRATIC_FINAL_STAGE
}

/** Normalize a possibly-absent / legacy persisted intake into a valid state: a
 *  pre-008 conversation (no intake, or any non-socratic mode) ⇒ Fast; a bad
 *  stage clamps into range. The single reader used everywhere a stored
 *  conversation's intake is consulted, so a cleared/corrupt store restarts
 *  cleanly (D1). */
export function normalizeIntake(raw: Partial<IntakeState> | null | undefined): IntakeState {
  if (!raw || raw.mode !== 'socratic') return fastIntake()
  return { mode: 'socratic', stage: clampStage(raw.stage ?? SOCRATIC_FIRST_STAGE) }
}
