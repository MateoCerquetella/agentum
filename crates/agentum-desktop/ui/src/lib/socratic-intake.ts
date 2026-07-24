// Spec 008 F2 + #257 — the Chat "Complex feature" Socratic interview is driven
// CLIENT-side (D1/D-B): the server is a pure `(mode, stage) → prompt` function
// and owns NO stage state. #257 made the progression ADAPTIVE: instead of
// blindly advancing one pass per user turn, the model ends each reply with a
// control marker (`[[socratic:advance|stay|done]]`) and the client moves the
// stage machine on it — re-running a pass whose answer was vague, and
// converging (`done`) only when the spec is actually well-defined. A reply
// with no marker (older server) falls back to the legacy one-pass advance.
// This module is the pure state machine — no React, no DOM — so the
// progression stays unit-testable in isolation.

/** Which intake experience a Chat message uses. Mirrors the server's
 *  `IntakeMode` (`routes/chat.rs`); the wire values are the snake_case strings. */
export type IntakeMode = 'fast' | 'socratic'

/** The five Socratic pass topics (spec 008 F2): WHO → WHAT → WHY →
 *  done-criteria → risks. `stage` travels in the request; the server maps it
 *  to a per-pass prompt. */
export const SOCRATIC_FIRST_STAGE = 1
export const SOCRATIC_FINAL_STAGE = 5

/** The per-conversation intake state (D1: rides the existing localStorage chat
 *  history, no new store table). `mode` is the experience; `stage` is the
 *  socratic pass the NEXT user turn will run (meaningful only for socratic);
 *  `converged` is set once the model signals `[[socratic:done]]` — the spec is
 *  defined and the composer can lean on "Preview issues". Absent on a
 *  conversation ⇒ Fast (back-compat with pre-008 chats). */
export type IntakeState = { mode: IntakeMode; stage: number; converged?: boolean }

/** The model's per-reply verdict on the interview (#257): `advance` = this
 *  pass's topic is covered, move on; `stay` = the answer was vague/missing,
 *  re-run the pass; `done` = the whole spec converged (final pass only). */
export type SocraticControl = 'advance' | 'stay' | 'done'

// The marker is machine-read and stripped before display. Tolerates optional
// whitespace inside the brackets and surrounding blank lines at the tail.
const SOCRATIC_CONTROL_RE = /\[\[\s*socratic\s*:\s*(advance|stay|done)\s*\]\]\s*$/i

/** Parse the trailing control marker out of a finished assistant reply.
 *  Returns null when no marker is present (Fast replies, older servers). */
export function parseSocraticControl(text: string): SocraticControl | null {
  const m = SOCRATIC_CONTROL_RE.exec(text.trimEnd())
  if (!m) return null
  return m[1].toLowerCase() as SocraticControl
}

/** Remove the trailing control marker (and the whitespace it rode on) so the
 *  transcript never shows the machine channel. Text without a marker passes
 *  through untouched. */
export function stripSocraticControl(text: string): string {
  return text.replace(/\s*\[\[\s*socratic\s*:\s*(advance|stay|done)\s*\]\]\s*$/i, '')
}

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

/** Advance the Socratic interview by exactly ONE pass — the legacy (marker-less)
 *  progression, kept as the fallback for replies from servers that don't emit
 *  the control marker. Fast never advances. Pure: `(state) → next state`. */
export function advanceIntake(state: IntakeState): IntakeState {
  if (state.mode !== 'socratic') return state
  return { mode: 'socratic', stage: clampStage(state.stage + 1) }
}

/** #257 — move the stage machine on a FINISHED assistant reply. `advance` steps
 *  one pass (never skips, capped at 5), `stay` re-runs the current pass, and
 *  `done` marks the interview converged (stage pins to the final pass). A
 *  reply with no marker falls back to the legacy one-pass advance so an older
 *  server keeps working. Fast states pass through untouched. */
export function resolveIntakeAfterReply(state: IntakeState, assistantText: string): IntakeState {
  if (state.mode !== 'socratic') return state
  switch (parseSocraticControl(assistantText)) {
    case 'stay':
      return { mode: 'socratic', stage: clampStage(state.stage) }
    case 'done':
      return { mode: 'socratic', stage: SOCRATIC_FINAL_STAGE, converged: true }
    case 'advance':
    case null:
      return advanceIntake(state)
  }
}

/** Whether the interview has converged — the point the composer stops advancing
 *  and leans on "Preview issues" (the convergence Fast shares). With markers
 *  this is the model's explicit `done`; a legacy state (no `converged` flag
 *  recorded) falls back to "reached the final pass". Always false for Fast. */
export function isSocraticComplete(state: IntakeState): boolean {
  if (state.mode !== 'socratic') return false
  if (state.converged !== undefined) return state.converged
  return clampStage(state.stage) >= SOCRATIC_FINAL_STAGE
}

/** Normalize a possibly-absent / legacy persisted intake into a valid state: a
 *  pre-008 conversation (no intake, or any non-socratic mode) ⇒ Fast; a bad
 *  stage clamps into range; `converged` survives only as an explicit boolean.
 *  The single reader used everywhere a stored conversation's intake is
 *  consulted, so a cleared/corrupt store restarts cleanly (D1). */
export function normalizeIntake(raw: Partial<IntakeState> | null | undefined): IntakeState {
  if (!raw || raw.mode !== 'socratic') return fastIntake()
  return {
    mode: 'socratic',
    stage: clampStage(raw.stage ?? SOCRATIC_FIRST_STAGE),
    ...(typeof raw.converged === 'boolean' ? { converged: raw.converged } : {})
  }
}
