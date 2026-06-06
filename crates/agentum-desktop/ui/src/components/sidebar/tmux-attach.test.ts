import { describe, it, expect } from 'vitest'
import { buildTmuxAttachCommand } from './tmux-attach'
import type { Session } from '@/runtime/agentum-server-client'
import type { ServerHost } from '@/runtime/server-host-client'

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: 's1',
    name: 'feature',
    workdir: '/tmp/repo',
    tool: 'claude',
    flags: [],
    status: 'running',
    tmux_target: 'agentum-feature-claude-abc',
    host_kind: 'local',
    created_at: '',
    updated_at: '',
    ...overrides
  } as Session
}

function host(overrides: Partial<ServerHost> = {}): ServerHost {
  return { id: 'h1', name: 'omarchy', kind: 'ssh', user: 'malloc', hostname: '172.30.66.4', port: 44444, ...overrides }
}

describe('buildTmuxAttachCommand', () => {
  it('returns a bare local attach for a local session', () => {
    expect(buildTmuxAttachCommand(session(), undefined)).toBe('tmux attach -t agentum-feature-claude-abc')
  })

  it('wraps in ssh -t with coords for an ssh session', () => {
    const s = session({ host_kind: 'ssh', host_id: 'h1' })
    expect(buildTmuxAttachCommand(s, host())).toBe(
      'ssh malloc@172.30.66.4 -p 44444 -t tmux attach -t agentum-feature-claude-abc'
    )
  })

  it('omits the -p flag for the default ssh port 22', () => {
    const s = session({ host_kind: 'ssh', host_id: 'h1' })
    expect(buildTmuxAttachCommand(s, host({ port: 22 }))).toBe(
      'ssh malloc@172.30.66.4 -t tmux attach -t agentum-feature-claude-abc'
    )
  })

  it('falls back to local form for an ssh session missing coords', () => {
    const s = session({ host_kind: 'ssh', host_id: 'h1' })
    expect(buildTmuxAttachCommand(s, undefined)).toBe('tmux attach -t agentum-feature-claude-abc')
  })

  it('returns null when there is no running tmux target', () => {
    expect(buildTmuxAttachCommand(session({ tmux_target: null }), undefined)).toBeNull()
    expect(buildTmuxAttachCommand(session({ tmux_target: '  ' }), undefined)).toBeNull()
  })

  it('uses the host kind when the session host_kind is absent', () => {
    const s = session({ host_kind: null, host_id: 'h1' })
    expect(buildTmuxAttachCommand(s, host())).toBe(
      'ssh malloc@172.30.66.4 -p 44444 -t tmux attach -t agentum-feature-claude-abc'
    )
  })
})
