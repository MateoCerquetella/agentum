// Regression coverage for the find-or-create logic in ensureWorkspaceSession.
//
// Bug: every git op (status, branch-compare, history, …) calls
// ensureWorkspaceSession({ workdir, tool: 'terminal' }) with no hostId. The
// server reports a LOCAL session's host_id as the nil UUID
// ("00000000-0000-0000-0000-000000000000"), NOT null. The host match compared
// `(s.host_id ?? null) === null`, which the nil-UUID string never satisfies —
// so an existing local session was never reused, and the deterministic-named
// createSession returned 409 AlreadyExists on every call after the first. That
// 409 ("agentum-server 409 on /api/sessions — {...}") then leaked into git UI
// panels ("Branch compare failed", GRAPH).
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('./agentum-server-client', () => ({
  listSessions: vi.fn(),
  createSession: vi.fn(),
  startSession: vi.fn()
}))

import { listSessions, createSession, startSession, type Session } from './agentum-server-client'
import { ensureWorkspaceSession } from './workspace-session'

const NIL_UUID = '00000000-0000-0000-0000-000000000000'

function session(overrides: Partial<Session>): Session {
  return {
    id: 'sess-1',
    name: 'repo-terminal-abc',
    workdir: '/repo',
    tool: 'terminal',
    flags: [],
    status: 'running',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides
  }
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe('ensureWorkspaceSession — local host matching', () => {
  it('reuses an existing local session reported with the nil-UUID host_id', async () => {
    // The server's wire shape for a local session: host_id is the nil UUID.
    const existing = session({ host_id: NIL_UUID, status: 'running' })
    vi.mocked(listSessions).mockResolvedValue([existing])

    const result = await ensureWorkspaceSession({ workdir: '/repo', tool: 'terminal' })

    // Must reuse, not create — otherwise the deterministic name collides → 409.
    expect(createSession).not.toHaveBeenCalled()
    expect(result.id).toBe('sess-1')
  })

  it('reuses an existing local session when the server omits host_id (null/undefined)', async () => {
    const existing = session({ host_id: null, status: 'running' })
    vi.mocked(listSessions).mockResolvedValue([existing])

    const result = await ensureWorkspaceSession({ workdir: '/repo', tool: 'terminal' })

    expect(createSession).not.toHaveBeenCalled()
    expect(result.id).toBe('sess-1')
  })
})

describe('ensureWorkspaceSession — create-time conflict (race)', () => {
  it('recovers from a 409 name conflict by reusing the racing session', async () => {
    // First list: nothing matches → we attempt to create. A concurrent caller
    // created the same deterministic-named session first, so createSession
    // throws the server's 409. We must re-list and reuse, not surface the 409.
    const racer = session({ host_id: NIL_UUID, status: 'running' })
    vi.mocked(listSessions)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([racer])
    vi.mocked(createSession).mockRejectedValue(
      new Error('agentum-server 409 on /api/sessions — {"error":"repo-terminal-abc"}')
    )

    const result = await ensureWorkspaceSession({ workdir: '/repo', tool: 'terminal' })

    expect(result.id).toBe('sess-1')
    // The 409 must not propagate to the caller (git panels).
    expect(startSession).not.toHaveBeenCalled() // racer already running
  })
})
