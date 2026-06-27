import { afterEach, describe, expect, it } from 'vitest'
import { useAppStore } from '@/store'
import { selectServerWorktreeActivity } from './server-worktree-activity'

const initialAppStoreState = useAppStore.getState()

afterEach(() => {
  useAppStore.setState(initialAppStoreState, true)
})

describe('server-worktree-activity slice', () => {
  it('selects alive + activity for a worktree, empty when unknown', () => {
    expect(selectServerWorktreeActivity(useAppStore.getState(), 'wt-1')).toEqual({ isAlive: false })

    useAppStore.getState().setServerWorktreeActivitySnapshot({
      'wt-1': { alive: true, activity: 'working' }
    })

    expect(selectServerWorktreeActivity(useAppStore.getState(), 'wt-1')).toEqual({
      isAlive: true,
      liveActivity: 'working'
    })
    expect(selectServerWorktreeActivity(useAppStore.getState(), 'wt-other')).toEqual({
      isAlive: false
    })
  })

  it('bumps sort + status epochs when the snapshot changes, and skips an identical refresh', () => {
    const before = useAppStore.getState()
    const sort0 = before.sortEpoch
    const status0 = before.agentStatusEpoch

    useAppStore.getState().setServerWorktreeActivitySnapshot({ 'wt-1': { alive: true } })
    const afterChange = useAppStore.getState()
    expect(afterChange.sortEpoch).toBe(sort0 + 1)
    expect(afterChange.agentStatusEpoch).toBe(status0 + 1)

    // Identical refresh — no epoch churn (avoids re-rendering every card on the
    // periodic heartbeat).
    useAppStore.getState().setServerWorktreeActivitySnapshot({ 'wt-1': { alive: true } })
    const afterNoop = useAppStore.getState()
    expect(afterNoop.sortEpoch).toBe(sort0 + 1)
    expect(afterNoop.agentStatusEpoch).toBe(status0 + 1)
  })

  it('patch marks a worktree alive and overlays activity from a live event', () => {
    useAppStore.getState().patchServerWorktreeActivity('wt-2', 'awaiting')
    expect(selectServerWorktreeActivity(useAppStore.getState(), 'wt-2')).toEqual({
      isAlive: true,
      liveActivity: 'awaiting'
    })

    // Re-patching the same verdict is a no-op for the epoch.
    const sort0 = useAppStore.getState().sortEpoch
    useAppStore.getState().patchServerWorktreeActivity('wt-2', 'awaiting')
    expect(useAppStore.getState().sortEpoch).toBe(sort0)
  })

  it('clear drops everything', () => {
    useAppStore.getState().setServerWorktreeActivitySnapshot({ 'wt-1': { alive: true } })
    useAppStore.getState().clearServerWorktreeActivity()
    expect(useAppStore.getState().serverWorktreeActivityByWorktreeId).toEqual({})
  })
})
