import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('CreateWorkspaceWizard issue drafting choices', () => {
  it('puts simple and complex drafting inside New workspace', () => {
    const source = readFileSync(new URL('./CreateWorkspaceWizard.tsx', import.meta.url), 'utf8')
    expect(source).toContain('Draft simple issue')
    expect(source).toContain('More drafting options')
    expect(source).toContain('Shape into spec…')
    expect(source).toContain('configured provider')
    expect(source).toContain('IssueSpecInterviewDialog')
  })

  it('returns the complex result to the worktree issue editor', () => {
    const source = readFileSync(new URL('./CreateWorkspaceWizard.tsx', import.meta.url), 'utf8')
    expect(source).toContain('createIssue.onApplyDraft(draft)')
    expect(source).toContain('value={createIssue.title}')
    expect(source).toContain('value={createIssue.body}')
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
