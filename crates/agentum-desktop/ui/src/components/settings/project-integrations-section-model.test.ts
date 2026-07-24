import { describe, expect, it } from 'vitest'
import { bindingFromLinearSelection } from './project-integrations-section-model'

describe('project integrations model', () => {
  it('persists exact workspace/project IDs instead of matching names', () => {
    expect(bindingFromLinearSelection({ id: 'workspace-id', organizationName: 'Same name' }, { id: 'project-id', name: 'Same name', url: 'https://linear.app/p' })).toMatchObject({ workspaceId: 'workspace-id', projectId: 'project-id' })
  })
})
