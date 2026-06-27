import { describe, expect, it } from 'vitest'
import {
  indexSessionsByWorktree,
  serverWorktreeActivityFromEvent
} from './server-worktree-activity-map'

const WT_A = { id: 'repo-1::/work/a', path: '/work/a' }
const WT_B = { id: 'repo-1::/work/b', path: '/work/b' }

describe('indexSessionsByWorktree', () => {
  it('marks a worktree alive when a backing session is running', () => {
    const { aliveWorktreeIds, sessionToWorktree } = indexSessionsByWorktree(
      [{ id: 's1', status: 'running', workdir: '/work/a' }],
      [WT_A, WT_B]
    )
    expect(aliveWorktreeIds).toEqual(['repo-1::/work/a'])
    expect(sessionToWorktree.get('s1')).toBe('repo-1::/work/a')
  })

  it('does not mark a worktree alive for a non-running session, but still indexes it', () => {
    const { aliveWorktreeIds, sessionToWorktree } = indexSessionsByWorktree(
      [{ id: 's1', status: 'idle', workdir: '/work/a' }],
      [WT_A]
    )
    expect(aliveWorktreeIds).toEqual([])
    // Still mapped so a later agent.* event for s1 can be routed to the worktree.
    expect(sessionToWorktree.get('s1')).toBe('repo-1::/work/a')
  })

  it('prefers worktree_path over workdir for board card-start sessions', () => {
    const { aliveWorktreeIds } = indexSessionsByWorktree(
      [{ id: 's1', status: 'running', workdir: '/repos/main', worktree_path: '/work/b' }],
      [WT_A, WT_B]
    )
    expect(aliveWorktreeIds).toEqual(['repo-1::/work/b'])
  })

  it('tolerates a trailing slash mismatch between session and worktree paths', () => {
    const { aliveWorktreeIds } = indexSessionsByWorktree(
      [{ id: 's1', status: 'running', workdir: '/work/a/' }],
      [WT_A]
    )
    expect(aliveWorktreeIds).toEqual(['repo-1::/work/a'])
  })

  it('ignores sessions whose workdir matches no known worktree', () => {
    const { aliveWorktreeIds, sessionToWorktree } = indexSessionsByWorktree(
      [{ id: 's1', status: 'running', workdir: '/somewhere/else' }],
      [WT_A]
    )
    expect(aliveWorktreeIds).toEqual([])
    expect(sessionToWorktree.size).toBe(0)
  })
})

describe('serverWorktreeActivityFromEvent', () => {
  it('maps watchdog event kinds to activity verdicts', () => {
    expect(serverWorktreeActivityFromEvent({ kind: 'agent.working', session_id: 's1' })).toEqual({
      sessionId: 's1',
      activity: 'working'
    })
    expect(
      serverWorktreeActivityFromEvent({ kind: 'agent.awaiting_input', session_id: 's1' })
    ).toEqual({ sessionId: 's1', activity: 'awaiting' })
    expect(serverWorktreeActivityFromEvent({ kind: 'agent.finished', session_id: 's1' })).toEqual({
      sessionId: 's1',
      activity: 'idle'
    })
  })

  it('resolves input_resolved to the resumed state', () => {
    expect(
      serverWorktreeActivityFromEvent({
        kind: 'agent.input_resolved',
        session_id: 's1',
        payload: { state: 'working' }
      })
    ).toEqual({ sessionId: 's1', activity: 'working' })
    expect(
      serverWorktreeActivityFromEvent({
        kind: 'agent.input_resolved',
        session_id: 's1',
        payload: { state: 'idle' }
      })
    ).toEqual({ sessionId: 's1', activity: 'idle' })
  })

  it('returns null for non-activity events or missing session id', () => {
    expect(serverWorktreeActivityFromEvent({ kind: 'session.started', session_id: 's1' })).toBeNull()
    expect(serverWorktreeActivityFromEvent({ kind: 'agent.working' })).toBeNull()
  })
})
