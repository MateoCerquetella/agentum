type HierarchySearchValue = string | null | undefined

/**
 * Small, allocation-light matcher shared by the live hierarchy and its tests.
 * Values are supplied by the caller so this stays independent from the large
 * renderer store types (workspace, project, host, terminal, and browser names
 * can all participate without coupling the model to Zustand).
 */
export function matchesSidebarHierarchySearch(
  query: string,
  values: readonly HierarchySearchValue[]
): boolean {
  const terms = query
    .trim()
    .toLocaleLowerCase()
    .split(/\s+/)
    .filter(Boolean)

  if (terms.length === 0) {
    return true
  }

  const haystack = values
    .filter((value): value is string => typeof value === 'string' && value.length > 0)
    .join('\n')
    .toLocaleLowerCase()

  return terms.every((term) => haystack.includes(term))
}
