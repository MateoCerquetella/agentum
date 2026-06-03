import { describe, expect, it } from 'vitest'
import {
  resolveMissingRepoProjectDialogState,
  resolveRepoBackedProjectDialogState
} from './project-dialog-state'

describe('resolveRepoBackedProjectDialogState', () => {
  it('keeps a repo-backed dialog when the repo still exists', () => {
    const dialog = { repoId: 'repo-1', label: 'Issue 1' }

    expect(resolveRepoBackedProjectDialogState(dialog, new Set(['repo-1']))).toBe(dialog)
  })

  it('clears a repo-backed dialog when its repo is removed', () => {
    expect(
      resolveRepoBackedProjectDialogState({ repoId: 'repo-1' }, new Set(['repo-2']))
    ).toBeNull()
  })
})

describe('resolveMissingRepoProjectDialogState', () => {
  it('waits for the slug index before closing missing-repo dialogs', () => {
    const slugDialog = { origin: { owner: 'mateocerquetella', repo: 'agentum' } }
    const repoNotInAgentum = { owner: 'mateocerquetella', repo: 'agentum', url: null }

    expect(
      resolveMissingRepoProjectDialogState({
        slugIndexReady: false,
        slugDialog,
        repoNotInAgentum,
        lookupSlug: () => ['repo-1']
      })
    ).toEqual({ slugDialog, repoNotInAgentum })
  })

  it('clears slug fallback dialogs once the repo slug resolves', () => {
    const slugDialog = { origin: { owner: 'mateocerquetella', repo: 'agentum' } }
    const repoNotInAgentum = { owner: 'other', repo: 'tool', url: null }
    const result = resolveMissingRepoProjectDialogState({
      slugIndexReady: true,
      slugDialog,
      repoNotInAgentum,
      lookupSlug: (slug) => (slug === 'mateocerquetella/agentum' ? ['repo-1'] : [])
    })

    expect(result.slugDialog).toBeNull()
    expect(result.repoNotInAgentum).toBe(repoNotInAgentum)
  })

  it('clears repo-not-in-agentum dialogs once the repo slug resolves', () => {
    const slugDialog = { origin: { owner: 'other', repo: 'tool' } }
    const repoNotInAgentum = { owner: 'mateocerquetella', repo: 'agentum', url: null }
    const result = resolveMissingRepoProjectDialogState({
      slugIndexReady: true,
      slugDialog,
      repoNotInAgentum,
      lookupSlug: (slug) => (slug === 'mateocerquetella/agentum' ? ['repo-1'] : [])
    })

    expect(result.slugDialog).toBe(slugDialog)
    expect(result.repoNotInAgentum).toBeNull()
  })
})
