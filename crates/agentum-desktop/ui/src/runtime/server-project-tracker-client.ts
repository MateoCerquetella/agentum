import type {
  ProjectTrackerConfig,
  ProjectTrackerConfigResponse,
  ProjectTrackerPreferences
} from '@/shared/project-tracker-config'
import { parseProjectTrackerConfig } from '@/shared/project-tracker-config'
import { apiUrl, getServerEndpoint } from './server-endpoint'

type ErrorPayload = {
  error?: unknown
  current?: unknown
}

export class ProjectTrackerConflictError extends Error {
  readonly current: ProjectTrackerConfig | null

  constructor(current: ProjectTrackerConfig | null) {
    super('Tracker settings changed elsewhere. Review the latest settings and try again.')
    this.name = 'ProjectTrackerConflictError'
    this.current = current
  }
}

async function authHeaders(): Promise<Record<string, string>> {
  const { token } = await getServerEndpoint()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

async function parseBody(response: Response): Promise<unknown> {
  const text = await response.text()
  if (!text) return undefined
  try {
    return JSON.parse(text) as unknown
  } catch {
    return text
  }
}

function errorMessage(status: number, body: unknown): string {
  if (typeof body === 'string' && body.trim()) return body
  if (typeof body === 'object' && body !== null) {
    const error = (body as ErrorPayload).error
    if (typeof error === 'string') return error
    if (typeof error === 'object' && error !== null) {
      const message = (error as { message?: unknown }).message
      if (typeof message === 'string') return message
    }
  }
  return `Tracker request failed (${status}).`
}

async function request(repoId: string, init?: RequestInit, query = ''): Promise<unknown> {
  const path = `/api/repos/${encodeURIComponent(repoId)}/tracker-config${query}`
  const response = await fetch(await apiUrl(path), {
    ...init,
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...(await authHeaders()),
      ...(init?.headers ?? {})
    }
  })
  const body = await parseBody(response)
  if (response.status === 409) {
    const currentValue =
      typeof body === 'object' && body !== null ? (body as ErrorPayload).current : null
    const current = currentValue == null ? null : parseProjectTrackerConfig(currentValue, repoId)
    throw new ProjectTrackerConflictError(current)
  }
  if (!response.ok) throw new Error(errorMessage(response.status, body))
  return body
}

export async function getProjectTrackerConfig(
  repoId: string
): Promise<ProjectTrackerConfigResponse> {
  const body = await request(repoId)
  if (typeof body !== 'object' || body === null) {
    throw new Error('Tracker config response is invalid.')
  }
  const response = body as { config?: unknown; migrationConflict?: unknown }
  return {
    config: response.config == null ? null : parseProjectTrackerConfig(response.config, repoId),
    ...(typeof response.migrationConflict === 'string'
      ? { migrationConflict: response.migrationConflict }
      : {})
  }
}

export async function putProjectTrackerConfig(
  repoId: string,
  config: ProjectTrackerConfig,
  expectedRevision: number | null
): Promise<ProjectTrackerConfig> {
  const body = await request(repoId, {
    method: 'PUT',
    body: JSON.stringify({ expectedRevision, config })
  })
  return parseProjectTrackerConfig(body, repoId)
}

export async function patchProjectTrackerPreferences(
  repoId: string,
  preferences: ProjectTrackerPreferences,
  expectedRevision: number
): Promise<ProjectTrackerConfig> {
  const body = await request(repoId, {
    method: 'PATCH',
    body: JSON.stringify({ expectedRevision, preferences })
  })
  return parseProjectTrackerConfig(body, repoId)
}

export async function deleteProjectTrackerConfig(
  repoId: string,
  expectedRevision: number | null
): Promise<void> {
  const query =
    expectedRevision === null ? '' : `?expectedRevision=${encodeURIComponent(expectedRevision)}`
  await request(repoId, { method: 'DELETE' }, query)
}
