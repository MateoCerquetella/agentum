// Regression tests for the "edited SSH target IP never reaches the server
// host" bug: the target→host linkage matched by SSH coords only, so changing
// the IP made the sync miss (the row still had the old hostname) and silently
// no-op — the daemon kept dialing the dead IP ("Operation timed out" on
// /api/hosts/{id}/tmux-sessions) and a later cold resolve created a duplicate
// host row that stranded the old row's sessions. The matcher must survive a
// coordinate change: cached id → current coords → pre-edit coords → host name.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SshTarget } from '../shared/ssh-types'

const { getJson, postJson, putJson, del, listTargets } = vi.hoisted(() => ({
  getJson: vi.fn(),
  postJson: vi.fn(),
  putJson: vi.fn(),
  del: vi.fn(),
  listTargets: vi.fn()
}))

vi.mock('./server-http', () => ({ getJson, postJson, putJson, del }))
vi.mock('@/tauri', () => ({ api: { ssh: { listTargets } } }))

type ClientModule = typeof import('./server-host-client')

const OLD_IP = '100.85.185.109'
const NEW_IP = '100.99.1.7'

function target(overrides: Partial<SshTarget> = {}): SshTarget {
  return {
    id: 'conn-1',
    label: 'dev-vps',
    host: OLD_IP,
    port: 22,
    username: 'me',
    ...overrides
  }
}

function hostRow(overrides: Record<string, unknown> = {}) {
  return {
    id: 'host-1',
    name: 'dev-vps',
    kind: 'ssh',
    user: 'me',
    hostname: OLD_IP,
    port: 22,
    ...overrides
  }
}

// The module caches connectionId → host id at module scope, so each test gets
// a fresh import for a cold cache (tests that want a warm cache seed it
// explicitly by resolving/syncing first).
async function loadClient(): Promise<ClientModule> {
  vi.resetModules()
  return import('./server-host-client')
}

beforeEach(() => {
  getJson.mockReset()
  postJson.mockReset()
  putJson.mockReset()
  listTargets.mockReset()
  putJson.mockImplementation(async (_path: string, body: { hostname: string }) =>
    hostRow({ hostname: body.hostname })
  )
})

describe('syncServerHostAuthForTarget after an edit', () => {
  it('still syncs an auth-only edit by unchanged coords', async () => {
    const client = await loadClient()
    getJson.mockResolvedValue([hostRow()])
    await client.syncServerHostAuthForTarget(target({ password: 'new-pw' }))
    expect(putJson).toHaveBeenCalledWith(
      '/api/hosts/host-1',
      expect.objectContaining({ hostname: OLD_IP, auth: { auth: 'password', password: 'new-pw' } })
    )
  })

  it('PUTs the new IP to the row already resolved for this connection (warm cache)', async () => {
    const client = await loadClient()
    getJson.mockResolvedValue([hostRow()])
    // Warm the connectionId → host id mapping the way the app does.
    listTargets.mockResolvedValue([target()])
    await client.resolveServerHostIdForConnection('conn-1')

    // The IP changes; the server row still holds OLD_IP so a coords match
    // would miss — the cached id must carry the update to the same row.
    await client.syncServerHostAuthForTarget(target({ host: NEW_IP }))
    expect(putJson).toHaveBeenLastCalledWith(
      '/api/hosts/host-1',
      expect.objectContaining({ hostname: NEW_IP })
    )
  })

  it('finds the row by the pre-edit coords when the cache is cold', async () => {
    const client = await loadClient()
    getJson.mockResolvedValue([hostRow({ name: 'renamed too' })])
    await client.syncServerHostAuthForTarget(
      target({ host: NEW_IP, label: 'new label' }),
      target() // pre-edit target: old IP + old label
    )
    expect(putJson).toHaveBeenCalledWith(
      '/api/hosts/host-1',
      expect.objectContaining({ hostname: NEW_IP })
    )
  })

  it('falls back to the host name when coords diverged in an earlier run', async () => {
    const client = await loadClient()
    getJson.mockResolvedValue([hostRow()])
    // No cache, no `previous` (e.g. a plain re-save after the app restarted):
    // the label is the last durable link to the row.
    await client.syncServerHostAuthForTarget(target({ host: NEW_IP }))
    expect(putJson).toHaveBeenCalledWith(
      '/api/hosts/host-1',
      expect.objectContaining({ hostname: NEW_IP })
    )
  })

  it('drops a stale cached mapping when no row matches', async () => {
    const client = await loadClient()
    getJson.mockResolvedValue([hostRow()])
    listTargets.mockResolvedValue([target()])
    await client.resolveServerHostIdForConnection('conn-1')

    // Host deleted server-side: sync must not PUT, and must evict the mapping
    // so the next resolve re-creates instead of returning the dead id.
    putJson.mockReset()
    getJson.mockResolvedValue([])
    await client.syncServerHostAuthForTarget(target({ label: 'other', host: NEW_IP }))
    expect(putJson).not.toHaveBeenCalled()

    postJson.mockResolvedValue(hostRow({ id: 'host-2', hostname: NEW_IP }))
    listTargets.mockResolvedValue([target({ host: NEW_IP })])
    await expect(client.resolveServerHostIdForConnection('conn-1')).resolves.toBe('host-2')
    expect(postJson).toHaveBeenCalled()
  })
})

describe('resolveServerHostIdForConnection after an IP change', () => {
  it('heals the existing row by name instead of creating a duplicate host', async () => {
    const client = await loadClient()
    // Cold cache (fresh app run): the native target already carries the new
    // IP, the server row still has the old one.
    listTargets.mockResolvedValue([target({ host: NEW_IP })])
    getJson.mockResolvedValue([hostRow()])

    await expect(client.resolveServerHostIdForConnection('conn-1')).resolves.toBe('host-1')
    expect(putJson).toHaveBeenCalledWith(
      '/api/hosts/host-1',
      expect.objectContaining({ hostname: NEW_IP })
    )
    expect(postJson).not.toHaveBeenCalled()
  })

  it('still creates a host when none exists', async () => {
    const client = await loadClient()
    listTargets.mockResolvedValue([target()])
    getJson.mockResolvedValue([])
    postJson.mockResolvedValue(hostRow())

    await expect(client.resolveServerHostIdForConnection('conn-1')).resolves.toBe('host-1')
    expect(postJson).toHaveBeenCalledWith(
      '/api/hosts',
      expect.objectContaining({ hostname: OLD_IP })
    )
  })
})
