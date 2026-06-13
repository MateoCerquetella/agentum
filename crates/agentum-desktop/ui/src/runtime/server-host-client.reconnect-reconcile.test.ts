// Unit test for the SSH-badge reconciliation a recovered session stream drives.
// Regression guard for the bug where a remote session's TERMINAL reconnected but
// the sidebar SSH badge + file tree stayed stuck on the outage: the recovery
// must flip the target to 'connected' (which bumps sshConnectedGeneration and
// re-fires the file explorer's failed-load retry). The drop must mark
// 'reconnecting' ONLY when currently connected, so each recovery is a real
// transition that bumps the generation again.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const setSshConnectionState = vi.fn()
const sshConnectionStates = new Map<string, { status: string }>()

vi.mock('@/store', () => ({
  useAppStore: { getState: () => ({ setSshConnectionState, sshConnectionStates }) }
}))
// Stub the modules server-host-client pulls in at load so no real IPC/HTTP runs.
vi.mock('@/tauri', () => ({ api: {} }))
vi.mock('./server-http', () => ({ getJson: vi.fn(), postJson: vi.fn(), putJson: vi.fn() }))

import {
  markHostConnectedFromHostKey,
  markHostReconnectingFromHostKey
} from './server-host-client'

beforeEach(() => {
  setSshConnectionState.mockClear()
  sshConnectionStates.clear()
})

afterEach(() => {
  vi.clearAllMocks()
})

describe('host reconnect reconciliation', () => {
  it('marks the target connected on recovery from an ssh hostKey', async () => {
    await markHostConnectedFromHostKey('ssh:target-123')
    expect(setSshConnectionState).toHaveBeenCalledWith('target-123', {
      targetId: 'target-123',
      status: 'connected',
      error: null,
      reconnectAttempt: 0
    })
  })

  it('is a no-op for local sessions and malformed host keys', async () => {
    await markHostConnectedFromHostKey(undefined)
    await markHostConnectedFromHostKey('local')
    await markHostConnectedFromHostKey('ssh:')
    expect(setSshConnectionState).not.toHaveBeenCalled()
  })

  it('marks reconnecting on drop ONLY when the target is currently connected', async () => {
    // Untracked host → don't fabricate a 'reconnecting' badge.
    await markHostReconnectingFromHostKey('ssh:t')
    expect(setSshConnectionState).not.toHaveBeenCalled()

    // A non-connected status (e.g. a failed explicit connect) is left alone.
    sshConnectionStates.set('t', { status: 'error' })
    await markHostReconnectingFromHostKey('ssh:t')
    expect(setSshConnectionState).not.toHaveBeenCalled()

    // Connected → downgrade to reconnecting so the next recovery re-transitions.
    sshConnectionStates.set('t', { status: 'connected' })
    await markHostReconnectingFromHostKey('ssh:t')
    expect(setSshConnectionState).toHaveBeenCalledWith('t', {
      targetId: 't',
      status: 'reconnecting',
      error: null,
      reconnectAttempt: 1
    })
  })
})
