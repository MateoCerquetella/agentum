import { describe, expect, it } from 'vitest'
import {
  formatWorkspaceCreateError,
  getWorkspaceCreateErrorToastMessage
} from './workspace-create-error-format'

describe('formatWorkspaceCreateError', () => {
  it('returns guidance for missing default base ref failures', () => {
    const error = new Error(
      'Could not resolve a default base ref for this repo. Pick a base branch explicitly and try again.'
    )

    const formatted = formatWorkspaceCreateError(error)

    expect(formatted).toEqual({
      title: 'No base branch found',
      message: 'Agentum could not resolve a usable base ref for this workspace.',
      help: 'Create an initial commit (for example on main), or select an existing branch in Create From, then try again.'
    })
    expect(getWorkspaceCreateErrorToastMessage(formatted)).toBe('No base branch found')
  })

  it('matches missing base ref failures case-insensitively', () => {
    const formatted = formatWorkspaceCreateError(
      new Error('COULD NOT RESOLVE A DEFAULT BASE REF from remote provider')
    )

    expect(formatted.title).toBe('No base branch found')
    expect(formatted.help).toBeDefined()
  })

  it('strips the noisy "fatal:" prefix from generic git errors', () => {
    const formatted = formatWorkspaceCreateError(new Error('fatal: not a git repository'))

    expect(formatted).toEqual({
      title: 'Could not create workspace',
      message: 'not a git repository'
    })
    expect(getWorkspaceCreateErrorToastMessage(formatted)).toBe('not a git repository')
  })

  it('explains an already-existing worktree folder and names it', () => {
    const formatted = formatWorkspaceCreateError(
      new Error(
        "Preparing worktree (checking out 'Test')\nfatal: '/Users/me/dev/repo/.claude/worktrees/Test' already exists"
      )
    )

    expect(formatted.title).toBe('That name is already taken')
    expect(formatted.message).toContain('Test')
    expect(formatted.help).toBeDefined()
  })

  it('explains a branch that is already checked out', () => {
    const formatted = formatWorkspaceCreateError(
      new Error("fatal: 'main' is already checked out at '/Users/me/dev/repo'")
    )

    expect(formatted.title).toBe('Branch already in use')
    expect(formatted.message).toContain('main')
  })

  it('explains an invalid base ref', () => {
    const formatted = formatWorkspaceCreateError(new Error('fatal: invalid reference: nope'))

    expect(formatted.title).toBe('Base branch not found')
  })

  it('falls back without crashing on a non-Error value', () => {
    const formatted = formatWorkspaceCreateError('boom')

    expect(formatted.title).toBe('Could not create workspace')
    expect(formatted.message).toBeDefined()
  })
})
