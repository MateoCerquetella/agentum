import type { LinearMode } from './linear-view-config'

/**
 * The Project Hub is a repo-scoped surface. Linear's generic Issues and Views
 * modes are account/workspace scoped, so restoring either one inside the hub
 * can render work from another Agentum project. Keep those modes available on
 * the standalone Tasks page, but fail closed to the project picker in the hub.
 */
export function resolveInitialLinearMode(
  embedded: boolean,
  persistedMode: LinearMode | undefined
): LinearMode {
  return embedded ? 'projects' : (persistedMode ?? 'issues')
}

export function linearModeOptionsForScope<T extends { id: LinearMode }>(
  options: readonly T[],
  embedded: boolean
): readonly T[] {
  return embedded ? options.filter((option) => option.id === 'projects') : options
}

export function shouldFetchLinearIssueLanding(
  embedded: boolean,
  linearMode: LinearMode
): boolean {
  return !embedded && linearMode === 'issues'
}
