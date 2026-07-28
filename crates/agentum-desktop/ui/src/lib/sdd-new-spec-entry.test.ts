// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { useAppStore } from '@/store'
import {
  consumeNewSpecPrefill,
  requestNewSpecFromWorkItem,
  subscribeNewSpecPrefill
} from './sdd-new-spec-entry'

describe('single New Spec work-item entry', () => {
  beforeEach(() => {
    vi.stubGlobal('crypto', { randomUUID: () => 'request-1' })
    useAppStore.setState({
      activeRepoId: null,
      activeView: 'activity',
      projectHubTab: 'tasks'
    })
    consumeNewSpecPrefill('repo-1')
  })

  it('routes GitHub identity to the project Specs page without launching work', () => {
    const intent = requestNewSpecFromWorkItem({
      repoId: 'repo-1',
      title: 'Refresh access tokens',
      provider: 'github',
      reference: 'https://github.com/acme/shop/issues/42'
    })

    expect(useAppStore.getState()).toMatchObject({
      activeRepoId: 'repo-1',
      activeView: 'project',
      projectHubTab: 'specs'
    })
    expect(intent).toMatchObject({
      requestId: 'request-1',
      sourceKind: 'github',
      sourceReference: 'https://github.com/acme/shop/issues/42'
    })
    expect(consumeNewSpecPrefill('another-repo')).toBeNull()
    expect(consumeNewSpecPrefill('repo-1')).toEqual(intent)
    expect(consumeNewSpecPrefill('repo-1')).toBeNull()
  })

  it('delivers a Linear prefill exactly once to the matching page subscriber', () => {
    const received: unknown[] = []
    const unsubscribe = subscribeNewSpecPrefill('repo-1', (intent) => received.push(intent))
    requestNewSpecFromWorkItem({
      repoId: 'repo-1',
      title: 'Keep sessions active',
      provider: 'linear',
      reference: 'ENG-123'
    })
    unsubscribe()

    expect(received).toHaveLength(1)
    expect(received[0]).toMatchObject({
      sourceKind: 'linear',
      sourceReference: 'ENG-123'
    })
    expect(consumeNewSpecPrefill('repo-1')).toBeNull()
  })

  it('keeps unsupported tracker provenance in Markdown instead of inventing an adapter', () => {
    const intent = requestNewSpecFromWorkItem({
      repoId: 'repo-1',
      title: 'GitLab follow-up',
      provider: 'unsupported',
      reference: 'https://gitlab.example/acme/shop/-/issues/9'
    })

    expect(intent.sourceKind).toBe('markdown')
    expect(intent.sourceReference).toBe('')
    expect(intent.goal).toContain('https://gitlab.example/acme/shop/-/issues/9')
  })
})
