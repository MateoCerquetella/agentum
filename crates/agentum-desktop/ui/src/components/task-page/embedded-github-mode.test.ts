import { describe, expect, it } from 'vitest'
import { resolveBoardProject } from '@/lib/board-project-resolution'
import { embeddedGithubModeForResolution } from './embedded-github-mode'

describe('embedded GitHub mode on repo switch', () => {
  it('shows the new repo picker when switching from a bound repo to an unbound repo', () => {
    const settings = { activeProject: null, activeProjectByRepo: {} }
    const agentum = resolveBoardProject({
      repoId: 'agentum',
      settings,
      bindingState: {
        status: 'loaded',
        binding: {
          projectOwner: 'MateoCerquetella',
          projectOwnerType: 'user',
          projectNumber: 2
        }
      }
    })
    const freebee = resolveBoardProject({
      repoId: 'freebee',
      settings,
      bindingState: { status: 'loaded', binding: null }
    })

    expect(agentum.project).toMatchObject({ owner: 'MateoCerquetella', number: 2 })
    expect(freebee).toEqual({ source: 'none', project: null })
    expect(embeddedGithubModeForResolution(agentum)).toBe('project')
    expect(embeddedGithubModeForResolution(freebee)).toBe('project')
  })
})
