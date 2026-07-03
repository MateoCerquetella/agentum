import {
  getRuntimePathBasename,
  isRuntimePathAbsolute,
  isWindowsAbsolutePathLike,
  normalizeRuntimePathForComparison,
  normalizeRuntimePathSeparators,
  relativePathInsideRoot,
  resolveRuntimePath
} from './cross-platform-path'
import { parseWslUncPath } from './wsl-paths'
import type {
  DetectedWorktree,
  ExternalWorktreeVisibility,
  GlobalSettings,
  AgentumWorkspaceLayout,
  Repo,
  Worktree,
  WorktreeMeta,
  WorktreeOwnership
} from './types'

export const EXTERNAL_WORKTREE_VISIBILITY_ROLLOUT_AT = Date.UTC(2026, 4, 23)

export function isLegacyRepoForExternalWorktreeVisibility(repo: Repo): boolean {
  if (typeof repo.externalWorktreeVisibilityLegacy === 'boolean') {
    return repo.externalWorktreeVisibilityLegacy
  }
  if (repo.externalWorktreeVisibility === undefined) {
    return true
  }
  if (!Number.isFinite(repo.addedAt)) {
    return true
  }
  return repo.addedAt < EXTERNAL_WORKTREE_VISIBILITY_ROLLOUT_AT
}

export function effectiveExternalWorktreeVisibility(
  repo: Pick<Repo, 'externalWorktreeVisibility'>,
  isLegacyRepoForVisibility: boolean
): ExternalWorktreeVisibility {
  if (repo.externalWorktreeVisibility) {
    return repo.externalWorktreeVisibility
  }
  return isLegacyRepoForVisibility ? 'show' : 'hide'
}

export function buildKnownAgentumWorkspaceLayouts(
  settings: Pick<GlobalSettings, 'workspaceDir' | 'nestWorkspaces' | 'workspaceDirHistory'>,
  repo?: Pick<Repo, 'path' | 'connectionId' | 'worktreeBasePath'>
): AgentumWorkspaceLayout[] {
  const layouts: AgentumWorkspaceLayout[] = []
  const repoBasePath = getRepoWorktreeBasePath(repo)
  if (repo && repoBasePath) {
    layouts.push({
      path: resolveWorkspaceLayoutPath(repo.path, repoBasePath),
      nestWorkspaces: settings.nestWorkspaces
    })
  }
  if (settings.workspaceDir && shouldIncludeWorkspaceLayout(repo, settings.workspaceDir)) {
    layouts.push({
      path: repo
        ? resolveWorkspaceLayoutPath(repo.path, settings.workspaceDir)
        : settings.workspaceDir,
      nestWorkspaces: settings.nestWorkspaces
    })
    appendWorkspaceLayouts(
      layouts,
      (settings.workspaceDirHistory ?? [])
        .filter((layout) => shouldIncludeWorkspaceLayout(repo, layout.path))
        .map((layout) => ({
          ...layout,
          path: repo ? resolveWorkspaceLayoutPath(repo.path, layout.path) : layout.path
        }))
    )
  }

  const wslLayouts = repo ? buildWslWorkspaceLayouts(repo.path, settings) : []
  appendWorkspaceLayouts(layouts, wslLayouts)

  const seen = new Set<string>()
  return layouts.filter((layout) => {
    const key = `${normalizeRuntimePathForComparison(layout.path)}:${layout.nestWorkspaces}`
    if (seen.has(key)) {
      return false
    }
    seen.add(key)
    return Boolean(layout.path)
  })
}

function appendWorkspaceLayouts(
  target: AgentumWorkspaceLayout[],
  source: readonly AgentumWorkspaceLayout[]
): void {
  // Why: workspace history is persisted user data and can grow large enough
  // for `push(...source)` to exceed the JavaScript call argument limit.
  for (const layout of source) {
    target.push(layout)
  }
}

function getRepoWorktreeBasePath(
  repo: Pick<Repo, 'worktreeBasePath'> | undefined
): string | undefined {
  const trimmed = repo?.worktreeBasePath?.trim()
  return trimmed || undefined
}

function resolveWorkspaceLayoutPath(repoPath: string, layoutPath: string): string {
  return isRuntimePathAbsoluteForRepo(repoPath, layoutPath)
    ? normalizeRuntimePathSeparators(layoutPath)
    : resolveRuntimePath(repoPath, layoutPath)
}

function isRuntimePathAbsoluteForRepo(repoPath: string, layoutPath: string): boolean {
  const pathFlavor =
    isWindowsAbsolutePathLike(repoPath) || isWindowsAbsolutePathLike(layoutPath)
      ? 'windows'
      : 'posix'
  return isRuntimePathAbsolute(layoutPath, pathFlavor)
}

function shouldIncludeWorkspaceLayout(
  repo: Pick<Repo, 'path' | 'connectionId'> | undefined,
  layoutPath: string
): boolean {
  return !repo?.connectionId || !isRuntimePathAbsoluteForRepo(repo.path, layoutPath)
}

function buildWslWorkspaceLayouts(
  repoPath: string,
  settings: Pick<GlobalSettings, 'nestWorkspaces' | 'workspaceDirHistory'>
): AgentumWorkspaceLayout[] {
  const parsed = parseWslUncPath(repoPath)
  if (!parsed) {
    return []
  }
  const homeMatch = parsed.linuxPath.match(/^\/home\/[^/]+(?:\/|$)/)
  const linuxHome = homeMatch?.[0].replace(/\/$/, '')
  if (!linuxHome) {
    return []
  }
  const root = `//wsl.localhost/${parsed.distro}${linuxHome}/agentum/workspaces`
  const historicalModes = (settings.workspaceDirHistory ?? []).map(
    (layout) => layout.nestWorkspaces
  )
  const modes = [settings.nestWorkspaces, ...historicalModes]
  return [...new Set(modes)].map((nestWorkspaces) => ({ path: root, nestWorkspaces }))
}

export function classifyWorktreeOwnership(args: {
  repo: Repo
  worktree: Pick<Worktree, 'path' | 'isMainWorktree'>
  meta?: WorktreeMeta
  settings: Pick<GlobalSettings, 'workspaceDir' | 'nestWorkspaces' | 'workspaceDirHistory'>
  knownAgentumLayouts: AgentumWorkspaceLayout[]
}): WorktreeOwnership {
  if (hasStrongAgentumMetadata(args.meta)) {
    return 'agentum-managed'
  }

  if (matchesStrongAgentumCreatePath(args.worktree.path, args.knownAgentumLayouts, args.repo)) {
    return 'agentum-managed'
  }

  if (isUnderFlatOrUntrustedAgentumRoot(args.worktree.path, args.knownAgentumLayouts)) {
    return 'unknown-legacy'
  }

  if (canClassifyAsExternal(args.worktree.path, args.knownAgentumLayouts)) {
    return 'external'
  }

  return 'unknown-legacy'
}

export function toDetectedWorktree(args: {
  repo: Repo
  worktree: Worktree
  meta?: WorktreeMeta
  settings: Pick<GlobalSettings, 'workspaceDir' | 'nestWorkspaces' | 'workspaceDirHistory'>
  knownAgentumLayouts: AgentumWorkspaceLayout[]
  isLegacyRepoForVisibility?: boolean
}): DetectedWorktree {
  const ownership = classifyWorktreeOwnership(args)
  const selectedCheckout = areRuntimePathsEqual(args.worktree.path, args.repo.path)
  const isLegacyRepoForVisibility =
    args.isLegacyRepoForVisibility ?? isLegacyRepoForExternalWorktreeVisibility(args.repo)
  const visible = shouldShowWorktree({
    worktree: args.worktree,
    ownership,
    repo: args.repo,
    isLegacyRepoForVisibility,
    isSelectedCheckout: selectedCheckout
  })

  return {
    ...args.worktree,
    ownership,
    selectedCheckout,
    visible
  }
}

export function shouldShowWorktree(args: {
  worktree: Pick<Worktree, 'path'>
  ownership: WorktreeOwnership
  repo: Repo
  isLegacyRepoForVisibility: boolean
  isSelectedCheckout: boolean
}): boolean {
  if (args.isSelectedCheckout) {
    return true
  }
  if (args.ownership === 'agentum-managed') {
    return true
  }
  if (args.ownership === 'unknown-legacy' && args.isLegacyRepoForVisibility) {
    return true
  }
  return effectiveExternalWorktreeVisibility(args.repo, args.isLegacyRepoForVisibility) === 'show'
}

function areRuntimePathsEqual(leftPath: string, rightPath: string): boolean {
  return (
    normalizeRuntimePathForComparison(leftPath) === normalizeRuntimePathForComparison(rightPath)
  )
}

function hasStrongAgentumMetadata(meta: WorktreeMeta | undefined): boolean {
  return Boolean(
    meta?.agentumCreatedAt ||
    meta?.createdAt ||
    meta?.createdWithAgent ||
    meta?.pushTarget ||
    meta?.sparseBaseRef ||
    meta?.sparsePresetId ||
    meta?.preserveBranchOnDelete
  )
}

function matchesStrongAgentumCreatePath(
  worktreePath: string,
  knownAgentumLayouts: readonly AgentumWorkspaceLayout[],
  repo: Pick<Repo, 'path'>
): boolean {
  const repoName = getRuntimePathBasename(repo.path).replace(/\.git$/i, '')
  if (!repoName) {
    return false
  }
  for (const layout of knownAgentumLayouts) {
    if (!layout.nestWorkspaces) {
      continue
    }
    const relative = relativePathInsideRoot(layout.path, worktreePath)
    if (relative === null) {
      continue
    }
    const segments = splitNormalizedPath(relative)
    const caseInsensitive =
      isWindowsAbsolutePathLike(layout.path) || isWindowsAbsolutePathLike(worktreePath)
    if (
      segments.length === 2 &&
      normalizePathSegment(segments[0], caseInsensitive) ===
        normalizePathSegment(repoName, caseInsensitive) &&
      segments[1].length > 0
    ) {
      return true
    }
  }
  return false
}

function isUnderFlatOrUntrustedAgentumRoot(
  worktreePath: string,
  knownAgentumLayouts: AgentumWorkspaceLayout[]
): boolean {
  for (const layout of knownAgentumLayouts) {
    const relative = relativePathInsideRoot(layout.path, worktreePath)
    if (relative === null) {
      continue
    }
    if (!layout.nestWorkspaces) {
      return true
    }
  }
  return false
}

function canClassifyAsExternal(
  worktreePath: string,
  knownAgentumLayouts: AgentumWorkspaceLayout[]
): boolean {
  if (knownAgentumLayouts.length === 0) {
    return false
  }
  for (const layout of knownAgentumLayouts) {
    const relative = relativePathInsideRoot(layout.path, worktreePath)
    if (relative === null) {
      continue
    }
    return layout.nestWorkspaces
  }
  return true
}

function splitNormalizedPath(value: string): string[] {
  return normalizeRuntimePathSeparators(value).split('/').filter(Boolean)
}

function normalizePathSegment(value: string, caseInsensitive: boolean): string {
  const normalized = normalizeRuntimePathSeparators(value)
  return caseInsensitive ? normalized.toLowerCase() : normalized
}
