import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import type { LatestAgentActivity } from './worktree-latest-activity'

const activityMock = vi.fn<() => LatestAgentActivity>()
vi.mock('./useLatestAgentActivity', () => ({
  useLatestAgentActivity: () => activityMock()
}))

import { SessionActivityCard } from './SessionActivityCard'

describe('SessionActivityCard', () => {
  beforeEach(() => activityMock.mockReset())

  it('renders the last message and tool call', () => {
    activityMock.mockReturnValue({
      lastAssistantMessage: 'Wired the worktree help',
      toolName: 'Bash',
      toolInput: 'cargo clippy --all-targets'
    })
    const markup = renderToStaticMarkup(
      React.createElement(SessionActivityCard, { worktreeId: 'w1' })
    )
    expect(markup).toContain('Wired the worktree help')
    expect(markup).toContain('Bash cargo clippy --all-targets')
  })

  it('renders nothing when there is no activity', () => {
    activityMock.mockReturnValue({})
    const result = SessionActivityCard({ worktreeId: 'w1' })
    expect(result).toBeNull()
  })
})
