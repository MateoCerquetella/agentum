import { describe, expect, it } from 'vitest'
import { getRepoRowDisplay } from './RepoCombobox'

describe('getRepoRowDisplay', () => {
  const repo = { id: 'r1', path: '/home/me/projects/finanzas' }

  it('renders an enabled row showing the repo path when not disabled', () => {
    expect(getRepoRowDisplay(repo)).toEqual({
      isDisabled: false,
      detailText: '/home/me/projects/finanzas'
    })
  })

  it('renders an enabled row when the disabled map has no entry for the repo', () => {
    const disabled = new Map<string, string>([['other', 'not a git repository on forge']])
    expect(getRepoRowDisplay(repo, disabled)).toEqual({
      isDisabled: false,
      detailText: '/home/me/projects/finanzas'
    })
  })

  it('renders a disabled row showing the reason in place of the path', () => {
    const disabled = new Map<string, string>([['r1', 'not a git repository on forge']])
    expect(getRepoRowDisplay(repo, disabled)).toEqual({
      isDisabled: true,
      detailText: 'not a git repository on forge'
    })
  })

  it('treats an empty-string reason as disabled (presence, not truthiness)', () => {
    const disabled = new Map<string, string>([['r1', '']])
    expect(getRepoRowDisplay(repo, disabled)).toEqual({ isDisabled: true, detailText: '' })
  })
})
