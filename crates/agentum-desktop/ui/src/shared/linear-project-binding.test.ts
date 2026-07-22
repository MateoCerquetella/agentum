import { describe, expect, it } from 'vitest'
import { normalizeLinearProjectBinding } from './linear-project-binding'

describe('normalizeLinearProjectBinding', () => {
  it('trims and freezes an exact binding', () => {
    const value = normalizeLinearProjectBinding({ workspaceId: ' w ', workspaceName: ' W ', projectId: ' p ', projectName: ' P ', projectUrl: 'https://linear.app/p' })
    expect(value).toEqual({ workspaceId: 'w', workspaceName: 'W', projectId: 'p', projectName: 'P', projectUrl: 'https://linear.app/p' })
    expect(Object.isFrozen(value)).toBe(true)
  })
  it('accepts null but rejects incomplete and unsafe values', () => {
    expect(normalizeLinearProjectBinding(null)).toBeNull()
    expect(normalizeLinearProjectBinding({ workspaceId: 'w', projectId: 'p' })).toBeUndefined()
    expect(normalizeLinearProjectBinding({ workspaceId: 'w', workspaceName: 'W', projectId: 'p', projectName: 'P', projectUrl: 'http://linear.app/p' })).toBeUndefined()
  })
})
