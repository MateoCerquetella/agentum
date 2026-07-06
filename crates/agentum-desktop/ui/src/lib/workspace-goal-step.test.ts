import { describe, expect, it } from 'vitest'
import {
  DEFAULT_COMPOSER_MODAL_PHASE,
  OPTIONAL_WORKSPACE_STEPS,
  deriveGoalIssueDraft,
  deriveWorkspaceGoalSeed,
  firstGoalStepBlocker,
  initialComposerPhase,
  isGoalStepReady,
  revealDetails,
  shouldStartAtGoalStep,
  slugifyGoalName
} from './workspace-goal-step'

// Spec 008 F3 (AC 9–11): pin the three gradeable behaviors of goal-first
// workspace creation — the goal→seed mapping (AC 9), the required-vs-optional
// inputs (AC 10), and the default-first-screen + "Skip to details" reveal
// decision (AC 9). All pure so they run without a DOM.

describe('slugifyGoalName (AC 9 seed → workspace name)', () => {
  it('lowercases, collapses punctuation, and joins the first words with "-"', () => {
    expect(slugifyGoalName('Add a dark-mode toggle to Settings!')).toBe('add-a-dark-mode-toggle-to')
  })

  it('drops emoji / non-slug characters instead of leaking them into the name', () => {
    expect(slugifyGoalName('Fix the 🔥 login bug')).toBe('fix-the-login-bug')
  })

  it('returns empty for a blank goal so the composer keeps its own default name', () => {
    expect(slugifyGoalName('')).toBe('')
    expect(slugifyGoalName('   \n  ')).toBe('')
  })

  it('clamps to a short, boundary-clean slug', () => {
    const slug = slugifyGoalName(
      'implement an extremely comprehensive multi tenant billing reconciliation subsystem'
    )
    expect(slug.length).toBeLessThanOrEqual(48)
    expect(slug.endsWith('-')).toBe(false)
    // maxWords caps the word count at 6.
    expect(slug.split('-').length).toBeLessThanOrEqual(6)
  })
})

describe('deriveWorkspaceGoalSeed (AC 9 "seed name/prompt from the goal")', () => {
  it('trims the goal and seeds a slug name + the verbatim prompt', () => {
    expect(deriveWorkspaceGoalSeed('  Add OAuth login  ')).toEqual({
      goal: 'Add OAuth login',
      name: 'add-oauth-login',
      prompt: 'Add OAuth login'
    })
  })

  it('keeps the prompt/goal verbatim even when the name slug is empty', () => {
    const seed = deriveWorkspaceGoalSeed('🚀🚀🚀')
    expect(seed.name).toBe('')
    expect(seed.prompt).toBe('🚀🚀🚀')
    expect(seed.goal).toBe('🚀🚀🚀')
  })
})

describe('deriveGoalIssueDraft (AC 11 tracker step pre-fill)', () => {
  it('uses the first line as the title and the whole goal as the body', () => {
    const draft = deriveGoalIssueDraft('Add OAuth login\n\nSupport GitHub and Google providers.')
    expect(draft.title).toBe('Add OAuth login')
    expect(draft.body).toBe('Add OAuth login\n\nSupport GitHub and Google providers.')
  })

  it('truncates an over-long title with an ellipsis but keeps the full body', () => {
    const longLine = 'a'.repeat(120)
    const draft = deriveGoalIssueDraft(longLine)
    expect(draft.title.length).toBe(72)
    expect(draft.title.endsWith('…')).toBe(true)
    expect(draft.body).toBe(longLine)
  })
})

describe('required vs optional inputs (AC 10, D9)', () => {
  it('requires BOTH goal and a workdir target (repoId)', () => {
    expect(isGoalStepReady({ goal: 'build a thing', repoId: 'repo-1' })).toBe(true)
    expect(isGoalStepReady({ goal: '', repoId: 'repo-1' })).toBe(false)
    expect(isGoalStepReady({ goal: 'build a thing', repoId: '' })).toBe(false)
    expect(isGoalStepReady({ goal: '   ', repoId: '   ' })).toBe(false)
  })

  it('names the first unmet required input, goal before workdir, never silent', () => {
    expect(firstGoalStepBlocker({ goal: '', repoId: '' })).toMatch(/goal/i)
    expect(firstGoalStepBlocker({ goal: 'build a thing', repoId: '' })).toMatch(/project|workdir/i)
    expect(firstGoalStepBlocker({ goal: 'build a thing', repoId: 'repo-1' })).toBeNull()
  })

  it('offers exactly the four skippable steps — none blocks creation', () => {
    // Spec 010 F3 appended `provision` as the FOURTH entry (the typed data
    // table is the designed extension seam); everything else is 008's pin.
    expect(OPTIONAL_WORKSPACE_STEPS.map((s) => s.id)).toEqual([
      'worktree',
      'scaffold',
      'tracker',
      'provision'
    ])
    // AC 10: every one of the four is skippable.
    expect(OPTIONAL_WORKSPACE_STEPS.every((s) => s.skippable === true)).toBe(true)
    // Reuse, don't rebuild: each names an existing primitive.
    expect(OPTIONAL_WORKSPACE_STEPS.map((s) => s.primitive)).toEqual([
      'createWorktree',
      'maybeScaffoldSpecFromIssue',
      'createGithubIssue',
      'provisionWorkspace'
    ])
  })
})

describe('default-first-screen + reveal decision (AC 9 / D3)', () => {
  it('defaults the plain create-workspace entry to the goal step', () => {
    expect(DEFAULT_COMPOSER_MODAL_PHASE).toBe('goal')
    expect(shouldStartAtGoalStep({})).toBe(true)
    expect(initialComposerPhase({})).toBe('goal')
    // A bare workdir preselect (sidebar "+" on a project) still starts goal-first.
    expect(initialComposerPhase({ prefilledName: '   ' })).toBe('goal')
  })

  it('skips the goal step for an opinionated open (protects F1 Tasks hop, D3 reach)', () => {
    // F1's Tasks-page pre-armed gated-run hop goes straight to details.
    expect(shouldStartAtGoalStep({ startGatedRun: true })).toBe(false)
    expect(initialComposerPhase({ startGatedRun: true })).toBe('details')
    // Create-from a linked item, a prefilled name, or a pinned base branch too.
    expect(shouldStartAtGoalStep({ linkedWorkItem: { number: 42 } })).toBe(false)
    expect(shouldStartAtGoalStep({ prefilledName: 'fix-login' })).toBe(false)
    expect(shouldStartAtGoalStep({ initialBaseBranch: 'main' })).toBe(false)
  })

  it('"Continue" reveals details seeded from the goal', () => {
    expect(revealDetails({ kind: 'continue', goal: '  Add OAuth login  ' })).toEqual({
      phase: 'details',
      seed: { goal: 'Add OAuth login', name: 'add-oauth-login', prompt: 'Add OAuth login' }
    })
  })

  it('"Skip to details" reveals details with NO seed (today\'s mechanics-first behavior)', () => {
    expect(revealDetails({ kind: 'skip' })).toEqual({ phase: 'details', seed: null })
  })
})
