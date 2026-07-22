import type { LinearProjectBinding, LinearProjectSummary, LinearWorkspace } from '@/shared/types'
import { normalizeLinearProjectBinding } from '@/shared/linear-project-binding'

export function bindingFromLinearSelection(
  workspace: Pick<LinearWorkspace, 'id' | 'organizationName'>,
  project: Pick<LinearProjectSummary, 'id' | 'name' | 'url'>
): LinearProjectBinding {
  const binding = normalizeLinearProjectBinding({
    workspaceId: workspace.id,
    workspaceName: workspace.organizationName,
    projectId: project.id,
    projectName: project.name,
    projectUrl: project.url
  })
  if (!binding) throw new Error('Linear workspace and project must have exact identities.')
  return binding
}
