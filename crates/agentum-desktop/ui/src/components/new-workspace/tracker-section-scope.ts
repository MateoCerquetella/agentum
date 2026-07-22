/**
 * TrackerSection's async commit boundary. Binding/table requests capture the
 * complete repo + resolved slug + Project scope; only the scope still rendered
 * by the component may commit status or row state.
 */
export function isCurrentTrackerSectionScope(
  capturedScopeKey: string,
  currentScopeKey: string | null
): boolean {
  return capturedScopeKey === currentScopeKey
}

/** Synchronous parent projection after the shared editor confirms DELETE. */
export function trackerSectionAfterSuccessfulUnbind(targetKey: string) {
  return {
    binding: { kind: 'absent' as const, targetKey },
    scopeKey: null
  }
}

export function trackerSectionTableForScope<T>(
  tableState: { scopeKey: string; table: T } | null,
  currentScopeKey: string | null,
  cachedTable: T | null
): T | null {
  return tableState?.scopeKey === currentScopeKey ? tableState.table : cachedTable
}

export function trackerConfigureActionLabel(hasResolvedProject: boolean): string {
  return hasResolvedProject ? 'Change tracker' : 'Configure tracker'
}
