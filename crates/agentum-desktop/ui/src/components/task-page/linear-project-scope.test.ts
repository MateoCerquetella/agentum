import { describe, expect, it } from 'vitest'

import {
  linearModeOptionsForScope,
  resolveInitialLinearMode,
  shouldFetchLinearIssueLanding
} from './linear-project-scope'

const options = [{ id: 'issues' }, { id: 'projects' }, { id: 'views' }] as const

describe('Linear Project Hub scope', () => {
  it('does not restore an account-wide mode inside a project', () => {
    expect(resolveInitialLinearMode(true, 'issues')).toBe('projects')
    expect(resolveInitialLinearMode(true, 'views')).toBe('projects')
    expect(resolveInitialLinearMode(false, 'views')).toBe('views')
  })

  it('only offers the project-bound mode inside a project', () => {
    expect(linearModeOptionsForScope(options, true).map((option) => option.id)).toEqual([
      'projects'
    ])
    expect(linearModeOptionsForScope(options, false)).toEqual(options)
  })

  it('never fetches the account-wide issue landing inside a project', () => {
    expect(shouldFetchLinearIssueLanding(true, 'issues')).toBe(false)
    expect(shouldFetchLinearIssueLanding(false, 'issues')).toBe(true)
    expect(shouldFetchLinearIssueLanding(false, 'projects')).toBe(false)
  })
})
