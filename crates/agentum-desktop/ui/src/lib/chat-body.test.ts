// Spec 009 (#361) AC-2: a workspace-selected send carries BOTH workdir and
// repo_id on the wire; without a workspace neither leaks in. Pure model tests
// over the extracted body builders — no fetch mock, no jsdom.
import { describe, expect, it } from 'vitest'

import { buildChatBody, buildChatStreamBody } from './chat-body'

const turns = [{ role: 'user' as const, content: 'how does the sidebar work?' }]

describe('buildChatBody', () => {
  it('includes workdir + repo_id when a workspace is selected', () => {
    const body = buildChatBody(turns, {
      workdir: '~/projects/agentum',
      repoId: 'repo-1',
      agent: 'codex'
    })
    expect(body.workdir).toBe('~/projects/agentum')
    expect(body.repo_id).toBe('repo-1')
    expect(body.agent).toBe('codex')
    expect(body.messages).toBe(turns)
  })

  it('omits repo identity entirely when no workspace is selected', () => {
    const json = JSON.stringify(buildChatBody(turns))
    // JSON.stringify drops undefined — the pre-009 wire shape survives.
    expect(json).not.toContain('repo_id')
    expect(json).not.toContain('workdir')
  })
})

describe('buildChatStreamBody', () => {
  it('carries repo identity plus the stream-only fields', () => {
    const body = buildChatStreamBody(turns, {
      workdir: '/home/u/proj',
      repoId: 'repo-2',
      model: 'm',
      thinking: true,
      mode: 'socratic',
      stage: 3,
      target: 'issue_spec',
      agent: 'codex'
    })
    expect(body.repo_id).toBe('repo-2')
    expect(body.workdir).toBe('/home/u/proj')
    expect(body.model).toBe('m')
    expect(body.thinking).toBe(true)
    expect(body.mode).toBe('socratic')
    expect(body.stage).toBe(3)
    expect(body.target).toBe('issue_spec')
    expect(body.agent).toBe('codex')
  })

  it('defaults thinking to false and drops absent repo identity', () => {
    const body = buildChatStreamBody(turns)
    expect(body.thinking).toBe(false)
    expect(JSON.stringify(body)).not.toContain('repo_id')
    expect(JSON.stringify(body)).not.toContain('target')
  })
})
