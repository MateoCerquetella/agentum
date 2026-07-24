export const PROJECT_TRACKER_SCHEMA_VERSION = 1 as const

export type ProjectTrackerProvider = 'github' | 'linear'
export type ProjectTrackerProvenance = 'configured' | 'migrated'

export type ProjectTrackerStatusMapping = {
  todo: string
  inProgress: string
  inReview: string
  readyToTest: string
  done: string
  blocked: string
}

export type ProjectTrackerBoardBinding = {
  projectId: string
  statusFieldId: string
  statusMapping: ProjectTrackerStatusMapping
  doneClosesIssue: boolean
  projectTitle?: string
  projectOwner?: string
  projectOwnerType?: string
  projectNumber?: number
  optionNames?: ProjectTrackerStatusMapping
}

export type ProjectTrackerGithubTarget = {
  repositorySlug: string
  projectBinding?: ProjectTrackerBoardBinding
}

export type ProjectTrackerLinearScope = {
  kind: 'project' | 'view'
  id: string
}

export type ProjectTrackerLinearTarget = {
  workspaceId: string
  teamId?: string
  scope?: ProjectTrackerLinearScope
}

export type ProjectTrackerProviderPreferences = {
  mode?: string
  preset?: string
  query?: string
  hiddenFieldIdsByView?: Record<string, string[]>
}

export type ProjectTrackerPreferences = {
  github?: ProjectTrackerProviderPreferences
  linear?: ProjectTrackerProviderPreferences
}

/** Canonical tracker ownership for one Agentum project. `repoId`, not a
 * checkout path or GitHub slug, is the durable cache and mutation key. */
export type ProjectTrackerConfig = {
  schemaVersion: typeof PROJECT_TRACKER_SCHEMA_VERSION
  repoId: string
  revision: number
  provider: ProjectTrackerProvider | null
  github?: ProjectTrackerGithubTarget
  linear?: ProjectTrackerLinearTarget
  taskPreferences: ProjectTrackerPreferences
  provenance: ProjectTrackerProvenance
}

export type ProjectTrackerConfigResponse = {
  config: ProjectTrackerConfig | null
  migrationConflict?: string
}

export function unconfiguredProjectTrackerConfig(
  repoId: string,
  current?: ProjectTrackerConfig | null
): ProjectTrackerConfig {
  return {
    schemaVersion: PROJECT_TRACKER_SCHEMA_VERSION,
    repoId,
    revision: current?.revision ?? 0,
    provider: null,
    taskPreferences: current?.taskPreferences ?? {},
    provenance: 'configured'
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function optionalString(value: unknown): value is string | undefined {
  return value === undefined || typeof value === 'string'
}

function isStatusMapping(value: unknown): value is ProjectTrackerStatusMapping {
  if (!isRecord(value)) return false
  return ['todo', 'inProgress', 'inReview', 'readyToTest', 'done', 'blocked'].every(
    (key) => typeof value[key] === 'string'
  )
}

function isBoardBinding(value: unknown): value is ProjectTrackerBoardBinding {
  if (!isRecord(value)) return false
  return (
    typeof value.projectId === 'string' &&
    typeof value.statusFieldId === 'string' &&
    isStatusMapping(value.statusMapping) &&
    typeof value.doneClosesIssue === 'boolean' &&
    optionalString(value.projectTitle) &&
    optionalString(value.projectOwner) &&
    optionalString(value.projectOwnerType) &&
    (value.projectNumber === undefined || typeof value.projectNumber === 'number') &&
    (value.optionNames === undefined || isStatusMapping(value.optionNames))
  )
}

/** Validate the server payload at the renderer boundary. This keeps a corrupt
 * or newer schema from masquerading as a configured provider in issue-creation
 * surfaces. */
export function parseProjectTrackerConfig(
  value: unknown,
  expectedRepoId?: string
): ProjectTrackerConfig {
  if (!isRecord(value)) throw new Error('Tracker config response is not an object.')
  if (value.schemaVersion !== PROJECT_TRACKER_SCHEMA_VERSION) {
    throw new Error('Tracker config uses an unsupported schema version.')
  }
  if (typeof value.repoId !== 'string' || (expectedRepoId && value.repoId !== expectedRepoId)) {
    throw new Error('Tracker config does not belong to the requested project.')
  }
  if (!Number.isInteger(value.revision) || (value.revision as number) < 0) {
    throw new Error('Tracker config revision is invalid.')
  }
  if (value.provider !== null && value.provider !== 'github' && value.provider !== 'linear') {
    throw new Error('Tracker config provider is invalid.')
  }
  if (!isRecord(value.taskPreferences) || !optionalString(value.provenance)) {
    throw new Error('Tracker config metadata is invalid.')
  }
  if (value.provenance !== 'configured' && value.provenance !== 'migrated') {
    throw new Error('Tracker config provenance is invalid.')
  }

  const github = value.github
  if (github !== undefined) {
    if (
      !isRecord(github) ||
      typeof github.repositorySlug !== 'string' ||
      (github.projectBinding !== undefined && !isBoardBinding(github.projectBinding))
    ) {
      throw new Error('GitHub tracker target is invalid.')
    }
  }
  const linear = value.linear
  if (linear !== undefined) {
    if (!isRecord(linear) || typeof linear.workspaceId !== 'string') {
      throw new Error('Linear tracker target is invalid.')
    }
    if (linear.teamId !== undefined && typeof linear.teamId !== 'string') {
      throw new Error('Linear tracker team is invalid.')
    }
    if (
      linear.scope !== undefined &&
      (!isRecord(linear.scope) ||
        (linear.scope.kind !== 'project' && linear.scope.kind !== 'view') ||
        typeof linear.scope.id !== 'string')
    ) {
      throw new Error('Linear tracker scope is invalid.')
    }
  }
  if (value.provider === 'github' && github === undefined) {
    throw new Error('Configured GitHub tracker has no target.')
  }
  if (value.provider === 'linear' && linear === undefined) {
    throw new Error('Configured Linear tracker has no target.')
  }
  if (value.provider === null && (github !== undefined || linear !== undefined)) {
    throw new Error('Unconfigured tracker carries an inactive provider target.')
  }

  return value as ProjectTrackerConfig
}
