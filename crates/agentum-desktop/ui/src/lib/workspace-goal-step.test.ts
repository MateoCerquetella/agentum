import { describe, expect, it } from 'vitest'
import { deriveGoalIssueDraft, slugifyGoalName } from './workspace-goal-step'

// Spec 013 F4 removed the goal-first phase machine; these pin the two pure
// transforms that survived it. All pure so they run without a DOM.

describe('slugifyGoalName (worktree-safe slug from free text)', () => {
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

describe('deriveGoalIssueDraft (spec 013 F2 intent → issue seed)', () => {
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
