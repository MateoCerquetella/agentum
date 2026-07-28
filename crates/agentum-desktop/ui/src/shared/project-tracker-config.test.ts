import { describe, expect, it } from 'vitest'
import {
  parseProjectTrackerConfig,
  unconfiguredProjectTrackerConfig
} from './project-tracker-config'

describe('project tracker config contract', () => {
  it('accepts a canonical Linear project target', () => {
    expect(
      parseProjectTrackerConfig(
        {
          schemaVersion: 1,
          repoId: 'repo-a',
          revision: 2,
          provider: 'linear',
          linear: {
            workspaceId: 'workspace-a',
            teamId: 'team-a',
            scope: { kind: 'project', id: 'project-a' }
          },
          taskPreferences: {},
          provenance: 'configured'
        },
        'repo-a'
      )
    ).toMatchObject({ provider: 'linear', revision: 2 })
  })

  it('fails closed when the selected provider has no matching target', () => {
    expect(() =>
      parseProjectTrackerConfig({
        schemaVersion: 1,
        repoId: 'repo-a',
        revision: 2,
        provider: 'github',
        taskPreferences: {},
        provenance: 'configured'
      })
    ).toThrow('has no target')
  })

  it('preserves preferences while explicitly configuring no tracker', () => {
    expect(
      unconfiguredProjectTrackerConfig('repo-a', {
        schemaVersion: 1,
        repoId: 'repo-a',
        revision: 4,
        provider: 'github',
        github: { repositorySlug: 'acme/widgets' },
        taskPreferences: { github: { query: 'label:ready' } },
        provenance: 'configured'
      })
    ).toEqual({
      schemaVersion: 1,
      repoId: 'repo-a',
      revision: 4,
      provider: null,
      taskPreferences: { github: { query: 'label:ready' } },
      provenance: 'configured'
    })
  })
})
