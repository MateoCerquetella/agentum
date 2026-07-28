import { describe, expect, it } from 'vitest'
import {
  canDraftIssue,
  canFileIssue,
  deriveCreateIssueIntentPhase,
  deriveDraftGroundingNote,
  deriveIntentTitle,
  deriveTrackerIntakePhase,
  resolveCreateIssueProvider
} from './create-issue-intent-model'
import type { FiledIssue } from './create-issue-intent-model'
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

// ---------- Spec 015 F3: Tracker-tab intake (add-only extensions) ----------

const FILED_GITHUB: FiledIssue = {
  provider: 'github',
  number: 42,
  url: 'https://github.com/acme/widgets/issues/42',
  slug: 'acme/widgets',
  title: 'Add dark mode'
}

const FILED_LINEAR: FiledIssue = {
  provider: 'linear',
  identifier: 'ENG-123',
  url: 'https://linear.app/acme/issue/ENG-123',
  title: 'Add dark mode'
}

describe('deriveTrackerIntakePhase', () => {
  const base = { generating: false, submitting: false, error: null, hasBody: false, filed: null }

  it('is idle before anything is drafted', () => {
    expect(deriveTrackerIntakePhase(base)).toBe('idle')
  })

  it('is drafting while the body is being generated', () => {
    expect(deriveTrackerIntakePhase({ ...base, generating: true })).toBe('drafting')
  })

  it('is review once a body has been drafted', () => {
    expect(deriveTrackerIntakePhase({ ...base, hasBody: true })).toBe('review')
  })

  it('is filing while the create call is in flight', () => {
    expect(deriveTrackerIntakePhase({ ...base, submitting: true, hasBody: true })).toBe('filing')
  })

  it('is error when the last op failed and nothing is in flight', () => {
    expect(deriveTrackerIntakePhase({ ...base, error: 'boom', hasBody: true })).toBe('error')
  })

  it('is filed after a provider-confirmed create', () => {
    expect(deriveTrackerIntakePhase({ ...base, filed: FILED_GITHUB })).toBe('filed')
    expect(deriveTrackerIntakePhase({ ...base, filed: FILED_LINEAR })).toBe('filed')
  })

  it('filed beats review — the drafted body is still in hand after a file', () => {
    expect(deriveTrackerIntakePhase({ ...base, hasBody: true, filed: FILED_GITHUB })).toBe('filed')
  })

  it('busy beats filed — a redraft/refile in flight shows its own phase', () => {
    expect(deriveTrackerIntakePhase({ ...base, generating: true, filed: FILED_GITHUB })).toBe(
      'drafting'
    )
    expect(deriveTrackerIntakePhase({ ...base, submitting: true, filed: FILED_GITHUB })).toBe(
      'filing'
    )
  })

  it('error beats filed — a failed follow-up op surfaces its banner', () => {
    expect(deriveTrackerIntakePhase({ ...base, error: 'boom', filed: FILED_GITHUB })).toBe('error')
  })

  it('busy beats a stale error (parity with the 013 phase model)', () => {
    expect(deriveTrackerIntakePhase({ ...base, generating: true, error: 'old' })).toBe('drafting')
    expect(
      deriveTrackerIntakePhase({ ...base, submitting: true, error: 'old', hasBody: true })
    ).toBe('filing')
  })
})

// ---------- Spec 020 F3: the honest grounding note (add-only) ----------

describe('deriveDraftGroundingNote', () => {
  it('is silent when the server sent no flag (a pre-020 response)', () => {
    expect(deriveDraftGroundingNote(null, 'vps')).toBeNull()
    expect(deriveDraftGroundingNote(null, null)).toBeNull()
  })

  it('is silent when the repo grounded the draft', () => {
    expect(deriveDraftGroundingNote({ repo: true, wiki: true }, null)).toBeNull()
    expect(deriveDraftGroundingNote({ repo: true, wiki: true }, 'vps')).toBeNull()
  })

  it('stays silent on a wiki-only miss — the normal local no-sidecar case', () => {
    expect(deriveDraftGroundingNote({ repo: true, wiki: false }, null)).toBeNull()
    expect(deriveDraftGroundingNote({ repo: true, wiki: false }, 'vps')).toBeNull()
  })

  it('notes a repo miss with the host label (SSH repo: files live elsewhere)', () => {
    expect(deriveDraftGroundingNote({ repo: false, wiki: true }, 'vps')).toBe(
      "Drafted without repo grounding — the repo's files live on vps."
    )
  })

  it('folds the wiki word in when BOTH sources were skipped', () => {
    expect(deriveDraftGroundingNote({ repo: false, wiki: false }, 'vps')).toBe(
      "Drafted without repo or wiki grounding — the repo's files live on vps."
    )
  })

  it('explains an unreadable local folder when no host label exists', () => {
    expect(deriveDraftGroundingNote({ repo: false, wiki: true }, null)).toBe(
      "Drafted without repo grounding — the project folder wasn't readable here."
    )
    expect(deriveDraftGroundingNote({ repo: false, wiki: false }, null)).toBe(
      "Drafted without repo or wiki grounding — the project folder wasn't readable here."
    )
  })
})
