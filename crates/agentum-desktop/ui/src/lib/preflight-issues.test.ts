import { describe, expect, it } from 'vitest'
import { getPreflightIssues } from './preflight-issues'

const ALL_OK = { git: { installed: true }, gh: { installed: true, authenticated: true } }

describe('getPreflightIssues', () => {
  it('returns no issues when git + gh are installed and authenticated', () => {
    expect(getPreflightIssues(ALL_OK)).toEqual([])
  })

  it('flags missing git', () => {
    const ids = getPreflightIssues({ ...ALL_OK, git: { installed: false } }).map((i) => i.id)
    expect(ids).toContain('git')
  })

  it('flags missing gh CLI (and not gh-auth)', () => {
    const ids = getPreflightIssues({
      ...ALL_OK,
      gh: { installed: false, authenticated: false }
    }).map((i) => i.id)
    expect(ids).toContain('gh')
    expect(ids).not.toContain('gh-auth')
  })

  it('flags unauthenticated gh when installed (and not gh)', () => {
    const ids = getPreflightIssues({
      ...ALL_OK,
      gh: { installed: true, authenticated: false }
    }).map((i) => i.id)
    expect(ids).toContain('gh-auth')
    expect(ids).not.toContain('gh')
  })
})
