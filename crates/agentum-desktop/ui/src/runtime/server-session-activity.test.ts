import { describe, expect, it, vi } from 'vitest'
import {
  activityRecordFromEvent,
  createServerSessionActivityHub,
  type ServerSessionActivityHandlers
} from './server-session-activity'

function makeHandlers(): ServerSessionActivityHandlers & {
  awaiting: ReturnType<typeof vi.fn>
  resolved: ReturnType<typeof vi.fn>
  working: ReturnType<typeof vi.fn>
  finished: ReturnType<typeof vi.fn>
} {
  const awaiting = vi.fn()
  const resolved = vi.fn()
  const working = vi.fn()
  const finished = vi.fn()
  return {
    awaiting,
    resolved,
    working,
    finished,
    onAwaitingInput: awaiting,
    onInputResolved: resolved,
    onWorking: working,
    onFinished: finished
  }
}

describe('activityRecordFromEvent', () => {
  it('maps the agent.* event kinds to activity records', () => {
    expect(activityRecordFromEvent({ kind: 'agent.awaiting_input' })).toEqual({
      kind: 'awaiting_input'
    })
    expect(activityRecordFromEvent({ kind: 'agent.working' })).toEqual({ kind: 'working' })
    expect(activityRecordFromEvent({ kind: 'agent.finished' })).toEqual({ kind: 'finished' })
    expect(
      activityRecordFromEvent({ kind: 'agent.input_resolved', payload: { state: 'working' } })
    ).toEqual({ kind: 'input_resolved', state: 'working' })
  })

  it('defaults input_resolved state to unknown when absent and ignores other kinds', () => {
    expect(activityRecordFromEvent({ kind: 'agent.input_resolved' })).toEqual({
      kind: 'input_resolved',
      state: 'unknown'
    })
    expect(activityRecordFromEvent({ kind: 'session.started' })).toBeNull()
    expect(activityRecordFromEvent({ kind: 'agent.hook' })).toBeNull()
    expect(activityRecordFromEvent({})).toBeNull()
  })
})

describe('createServerSessionActivityHub', () => {
  it('dispatches a live event to the registered handler for its session', () => {
    const hub = createServerSessionActivityHub()
    const h = makeHandlers()
    hub.register('sess-1', h)

    hub.handleEvent({ kind: 'agent.awaiting_input', session_id: 'sess-1' })
    expect(h.awaiting).toHaveBeenCalledTimes(1)

    hub.handleEvent({
      kind: 'agent.input_resolved',
      session_id: 'sess-1',
      payload: { state: 'working' }
    })
    expect(h.resolved).toHaveBeenCalledWith('working')
  })

  it('does not cross-deliver events to a different session', () => {
    const hub = createServerSessionActivityHub()
    const h = makeHandlers()
    hub.register('sess-1', h)

    hub.handleEvent({ kind: 'agent.awaiting_input', session_id: 'sess-OTHER' })
    expect(h.awaiting).not.toHaveBeenCalled()
  })

  it('replays the cached current state to a pane that registers after the event (reload seed)', () => {
    const hub = createServerSessionActivityHub()
    // The /api/events replay arrives BEFORE this pane mounts.
    hub.handleEvent({ kind: 'agent.awaiting_input', session_id: 'sess-1', payload: { replay: true } })

    const h = makeHandlers()
    hub.register('sess-1', h)
    // Registration immediately seeds the dot from the cached state.
    expect(h.awaiting).toHaveBeenCalledTimes(1)
  })

  it('caches only the latest state per session', () => {
    const hub = createServerSessionActivityHub()
    hub.handleEvent({ kind: 'agent.awaiting_input', session_id: 'sess-1' })
    hub.handleEvent({ kind: 'agent.working', session_id: 'sess-1' })

    const h = makeHandlers()
    hub.register('sess-1', h)
    expect(h.working).toHaveBeenCalledTimes(1)
    expect(h.awaiting).not.toHaveBeenCalled()
  })

  it('stops delivering after unregister', () => {
    const hub = createServerSessionActivityHub()
    const h = makeHandlers()
    const unregister = hub.register('sess-1', h)
    unregister()

    hub.handleEvent({ kind: 'agent.awaiting_input', session_id: 'sess-1' })
    expect(h.awaiting).not.toHaveBeenCalled()
    expect(hub.hasHandlers()).toBe(false)
  })

  it('unregister does not clobber a newer registration for the same session (remount race)', () => {
    const hub = createServerSessionActivityHub()
    const first = makeHandlers()
    const unregisterFirst = hub.register('sess-1', first)
    const second = makeHandlers()
    hub.register('sess-1', second)

    // The stale first unregister must not remove the live second handler.
    unregisterFirst()
    hub.handleEvent({ kind: 'agent.finished', session_id: 'sess-1' })
    expect(second.finished).toHaveBeenCalledTimes(1)
    expect(first.finished).not.toHaveBeenCalled()
  })
})
