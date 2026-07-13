import { describe, expect, it } from 'vitest'
import {
  canDraftIssue,
  canFileIssue,
  deriveCreateIssueIntentPhase,
  deriveIntentTitle,
  resolveCreateIssueProvider
} from './create-issue-intent-model'
import type { PickerProjectRef } from './work-item-picker-model'

const PROJECT: PickerProjectRef = { owner: 'acme', ownerType: 'organization', number: 7 }

describe('deriveCreateIssueIntentPhase', () => {
  it('is idle before anything is drafted', () => {
    expect(
      deriveCreateIssueIntentPhase({
        generating: false,
        submitting: false,
        error: null,
        hasBody: false
      })
    ).toBe('idle')
  })

  it('is drafting while the body is being generated', () => {
    expect(
      deriveCreateIssueIntentPhase({ generating: true, submitting: false, error: null, hasBody: false })
    ).toBe('drafting')
  })

  it('is review once a body has been drafted', () => {
    expect(
      deriveCreateIssueIntentPhase({ generating: false, submitting: false, error: null, hasBody: true })
    ).toBe('review')
  })

  it('is filing while the create call is in flight', () => {
    expect(
      deriveCreateIssueIntentPhase({ generating: false, submitting: true, error: null, hasBody: true })
    ).toBe('filing')
  })

  it('is error when the last op failed and nothing is in flight', () => {
    expect(
      deriveCreateIssueIntentPhase({
        generating: false,
        submitting: false,
        error: 'boom',
        hasBody: true
      })
    ).toBe('error')
  })

  it('lets an in-flight op win over a stale error (busy > error)', () => {
    // A retry: the old error is still set but a new draft/file is running.
    expect(
      deriveCreateIssueIntentPhase({ generating: true, submitting: false, error: 'old', hasBody: false })
    ).toBe('drafting')
    expect(
      deriveCreateIssueIntentPhase({ generating: false, submitting: true, error: 'old', hasBody: true })
    ).toBe('filing')
  })
})

describe('canDraftIssue', () => {
  it('requires a non-blank intent and no in-flight op', () => {
    expect(canDraftIssue('add dark mode', false)).toBe(true)
    expect(canDraftIssue('   ', false)).toBe(false)
    expect(canDraftIssue('', false)).toBe(false)
    expect(canDraftIssue('add dark mode', true)).toBe(false)
  })
})

describe('canFileIssue', () => {
  it('requires a title and no in-flight op', () => {
    expect(canFileIssue('Add dark mode', false)).toBe(true)
    expect(canFileIssue('   ', false)).toBe(false)
    expect(canFileIssue('Add dark mode', true)).toBe(false)
  })
})

describe('deriveIntentTitle', () => {
  it('seeds a title from the first line of the intent', () => {
    expect(deriveIntentTitle('Add dark mode\nmore detail here')).toBe('Add dark mode')
  })
  it('is blank for a blank intent (composer keeps its own default)', () => {
    expect(deriveIntentTitle('   ')).toBe('')
  })
  it('truncates an overlong first line', () => {
    const long = 'x'.repeat(120)
    const title = deriveIntentTitle(long)
    expect(title.length).toBeLessThanOrEqual(72)
    expect(title.endsWith('…')).toBe(true)
  })
})

describe('resolveCreateIssueProvider', () => {
  it('prefers the resolved GitHub Project, falls back to Linear, flags ambiguous', () => {
    // A resolved Project wins (follow the resolved tracker's provider).
    expect(resolveCreateIssueProvider({ resolved: PROJECT, linearConnected: false })).toBe('github')
    // No Project but Linear connected ⇒ Linear.
    expect(resolveCreateIssueProvider({ resolved: null, linearConnected: true })).toBe('linear')
    // Both ⇒ ambiguous (the sub-panel shows a toggle).
    expect(resolveCreateIssueProvider({ resolved: PROJECT, linearConnected: true })).toBe(
      'ambiguous'
    )
    // Neither ⇒ github (the default create path surfaces its own error inline).
    expect(resolveCreateIssueProvider({ resolved: null, linearConnected: false })).toBe('github')
  })
})
