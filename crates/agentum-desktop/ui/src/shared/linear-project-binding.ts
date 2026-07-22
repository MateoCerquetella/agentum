import type { LinearProjectBinding } from './types'

export function normalizeLinearProjectBinding(value: unknown): LinearProjectBinding | null | undefined {
  if (value === null) return null
  if (!value || typeof value !== 'object') return undefined
  const input = value as Record<string, unknown>
  const workspaceId = stringField(input.workspaceId)
  const workspaceName = stringField(input.workspaceName)
  const projectId = stringField(input.projectId)
  const projectName = stringField(input.projectName)
  if (!workspaceId || !workspaceName || !projectId || !projectName) return undefined

  const rawUrl = input.projectUrl
  let projectUrl: string | undefined
  if (rawUrl !== undefined) {
    projectUrl = stringField(rawUrl)
    if (!projectUrl) return undefined
    try {
      if (new URL(projectUrl).protocol !== 'https:') return undefined
    } catch {
      return undefined
    }
  }
  return Object.freeze({ workspaceId, workspaceName, projectId, projectName, ...(projectUrl ? { projectUrl } : {}) })
}

function stringField(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  return value.trim() || undefined
}
