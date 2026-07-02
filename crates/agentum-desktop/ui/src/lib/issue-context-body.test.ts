import { describe, expect, it } from 'vitest'
import { composeIssueContextBody } from './issue-context-body'

// Spec 006 F1 (AC 3): the exact assembly contract — both-blank stays bodyless
// (no new failure mode), anything else renders under `## Context`.
describe('composeIssueContextBody', () => {
  it('returns undefined when both fields are blank', () => {
    expect(composeIssueContextBody('', '')).toBeUndefined()
  })

  it('renders the prompt alone under the Context heading', () => {
    expect(composeIssueContextBody('fix the parser', '')).toBe('## Context\n\nfix the parser')
  })

  it('renders the note alone as a Note line', () => {
    expect(composeIssueContextBody('', 'from PR #7')).toBe('## Context\n\n**Note:** from PR #7')
  })

  it('renders both sections in prompt-then-note order with no trailing newline', () => {
    expect(composeIssueContextBody('fix the parser', 'from PR #7')).toBe(
      '## Context\n\nfix the parser\n\n**Note:** from PR #7'
    )
  })

  it('treats whitespace-only inputs as blank', () => {
    expect(composeIssueContextBody('   \n\t', '  ')).toBeUndefined()
  })
})
