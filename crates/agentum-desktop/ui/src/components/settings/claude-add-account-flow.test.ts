import { describe, expect, it } from 'vitest'
import {
  decideClaudeAddAccount,
  isClaudeLoginCaptureReady,
  type ClaudeLiveLogin
} from './claude-add-account-flow'

const signedIn = (email: string | null): ClaudeLiveLogin => ({ hasCredentials: true, email })
const signedOut: ClaudeLiveLogin = { hasCredentials: false, email: null }

describe('decideClaudeAddAccount', () => {
  it('captures directly when the live login is not saved yet', () => {
    expect(decideClaudeAddAccount(signedIn('a@example.com'), [])).toEqual({ kind: 'capture' })
    expect(decideClaudeAddAccount(signedIn('a@example.com'), ['b@example.com'])).toEqual({
      kind: 'capture'
    })
  })

  it('offers the sign-out hand-off when the live login is already saved', () => {
    expect(decideClaudeAddAccount(signedIn('a@example.com'), ['a@example.com'])).toEqual({
      kind: 'confirm-signout',
      email: 'a@example.com'
    })
  })

  it('matches saved emails case-insensitively', () => {
    expect(decideClaudeAddAccount(signedIn('A@Example.com'), ['a@example.com'])).toEqual({
      kind: 'confirm-signout',
      email: 'A@Example.com'
    })
  })

  it('captures when credentials exist but no identity email is available', () => {
    // No oauthAccount block: dedupe cannot match, so capture (backend falls
    // back to a generic label and still upserts by that label).
    expect(decideClaudeAddAccount(signedIn(null), ['a@example.com'])).toEqual({ kind: 'capture' })
  })

  it('waits for a sign-in when the machine is signed out', () => {
    expect(decideClaudeAddAccount(signedOut, ['a@example.com'])).toEqual({
      kind: 'wait-for-login'
    })
  })
})

describe('isClaudeLoginCaptureReady', () => {
  it('requires both credentials and the identity email', () => {
    expect(isClaudeLoginCaptureReady(signedOut)).toBe(false)
    expect(isClaudeLoginCaptureReady(signedIn(null))).toBe(false)
    expect(isClaudeLoginCaptureReady(signedIn('a@example.com'))).toBe(true)
  })
})
