import { describe, expect, it } from 'vitest'
import {
  buildWorktreeActivitySnapshot,
  indexSessionsByWorktree,
  serverWorktreeActivityFromEvent,
  type ServerWorktreeLiveActivity
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

  it('tolerates a path-less worktree without throwing and still maps the others', () => {
    // A degraded/failed detection (or unreachable remote host) can leave a
    // worktree with no `path`. One undefined path must not abort the whole
    // index — it would blank EVERY sidebar dot, not just that worktree's.
    const pathless = { id: 'repo-1::', path: undefined as unknown as string }
    const { aliveWorktreeIds, sessionToWorktree } = indexSessionsByWorktree(
      [{ id: 's1', status: 'running', workdir: '/work/a' }],
      [pathless, WT_A]
    )
    expect(aliveWorktreeIds).toEqual(['repo-1::/work/a'])
    expect(sessionToWorktree.get('s1')).toBe('repo-1::/work/a')
  })

  it('ignores a session with an empty workdir (never binds it to a path-less worktree)', () => {
    const { aliveWorktreeIds, sessionToWorktree } = indexSessionsByWorktree(
      [{ id: 's1', status: 'running', workdir: '' }],
      [WT_A]
    )
    expect(aliveWorktreeIds).toEqual([])
    expect(sessionToWorktree.size).toBe(0)
  })
})

describe('buildWorktreeActivitySnapshot', () => {
  const idx = (pairs: [string, string][]): Map<string, string> => new Map(pairs)
  const acts = (pairs: [string, ServerWorktreeLiveActivity][]): Map<string, ServerWorktreeLiveActivity> =>
    new Map(pairs)

  it('marks alive worktrees with the {alive:true} baseline and no activity', () => {
    const snap = buildWorktreeActivitySnapshot(['wt-1'], idx([]), acts([]))
    expect(snap).toEqual({ 'wt-1': { alive: true } })
  })

  it('keeps a working agent over an idle sibling in the same worktree', () => {
    // The reload "stuck idle" bug: two sessions back one worktree; a plain
    // last-writer-wins overlay let the idle one clobber the working one.
    const snap = buildWorktreeActivitySnapshot(
      ['wt-1'],
      idx([
        ['s-working', 'wt-1'],
        ['s-idle', 'wt-1']
      ]),
      acts([
        ['s-working', 'working'],
        ['s-idle', 'idle']
      ])
    )
    expect(snap['wt-1']).toEqual({ alive: true, activity: 'working' })
  })

  it('is order-independent (idle inserted last cannot clobber working)', () => {
    const snap = buildWorktreeActivitySnapshot(
      ['wt-1'],
      idx([
        ['s-idle', 'wt-1'],
        ['s-working', 'wt-1']
      ]),
      acts([
        ['s-idle', 'idle'],
        ['s-working', 'working']
      ])
    )
    expect(snap['wt-1']).toEqual({ alive: true, activity: 'working' })
  })

  it('ranks awaiting (needs you) above working', () => {
    const snap = buildWorktreeActivitySnapshot(
      ['wt-1'],
      idx([
        ['s-working', 'wt-1'],
        ['s-awaiting', 'wt-1']
      ]),
      acts([
        ['s-working', 'working'],
        ['s-awaiting', 'awaiting']
      ])
    )
    expect(snap['wt-1']).toEqual({ alive: true, activity: 'awaiting' })
  })

  it('marks a worktree alive from a verdict even if it was not in the alive set', () => {
    const snap = buildWorktreeActivitySnapshot([], idx([['s1', 'wt-1']]), acts([['s1', 'working']]))
    expect(snap['wt-1']).toEqual({ alive: true, activity: 'working' })
  })

  it('drops a verdict for a session with no worktree mapping', () => {
    const snap = buildWorktreeActivitySnapshot(['wt-1'], idx([]), acts([['s-orphan', 'working']]))
    expect(snap).toEqual({ 'wt-1': { alive: true } })
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
