import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('TrackerIntakePanel drafting choices', () => {
  it('keeps the primary quick action and exposes the focused spec path in a menu', () => {
    const source = readFileSync(new URL('./TrackerIntakePanel.tsx', import.meta.url), 'utf8')
    expect(source).toContain('Draft issue')
    expect(source).toContain('More drafting options')
    expect(source).toContain('Shape into spec…')
    expect(source).toContain('IssueSpecInterviewDialog')
  })

  it('returns the complex result to the existing editable intake draft', () => {
    const source = readFileSync(new URL('./TrackerIntakePanel.tsx', import.meta.url), 'utf8')
    expect(source).toContain('intake.applyDraft(draft)')
    expect(source).toContain('value={intake.title}')
    expect(source).toContain('value={intake.body}')
  })
})

describe('IssueSpecInterviewDialog contract', () => {
  it('reuses the adaptive five-pass interview and previews without filing', () => {
    const source = readFileSync(new URL('./IssueSpecInterviewDialog.tsx', import.meta.url), 'utf8')
    expect(source).toContain("target: 'issue_spec'")
    expect(source).toContain("mode: 'socratic'")
    expect(source).toContain('previewIssueSpec')
    expect(source).toContain('Review issue')
    expect(source).not.toContain('createGithubIssue')
    expect(source).not.toContain('linearCreateIssue')
  })
})
