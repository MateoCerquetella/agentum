export function visibleOperationalProjectCount(args: {
  availableWidth: number
  projectWidths: readonly number[]
  reservedWidth?: number
}): number {
  let remaining = Math.max(0, args.availableWidth - (args.reservedWidth ?? 96))
  let count = 0
  for (const width of args.projectWidths) {
    const required = Math.max(48, width) + 6
    if (required > remaining) break
    remaining -= required
    count++
  }
  return count
}

export function orderOperationalProjects<
  T extends { id: string; displayName: string }
>(args: {
  repos: readonly T[]
  selectedRepoIds: readonly string[]
  activeRepoId?: string
  workspaceCountByRepoId?: ReadonlyMap<string, number>
}): T[] {
  const selectedIndex = new Map(args.selectedRepoIds.map((id, index) => [id, index]))
  const originalIndex = new Map(args.repos.map((repo, index) => [repo.id, index]))

  return [...args.repos].sort((a, b) => {
    const aSelected = selectedIndex.get(a.id)
    const bSelected = selectedIndex.get(b.id)
    if (aSelected !== undefined || bSelected !== undefined) {
      if (aSelected === undefined) return 1
      if (bSelected === undefined) return -1
      return aSelected - bSelected
    }

    const aActive = a.id === args.activeRepoId
    const bActive = b.id === args.activeRepoId
    if (aActive !== bActive) return aActive ? -1 : 1

    const byWorkspaceCount =
      (args.workspaceCountByRepoId?.get(b.id) ?? 0) -
      (args.workspaceCountByRepoId?.get(a.id) ?? 0)
    if (byWorkspaceCount !== 0) return byWorkspaceCount

    const byName = a.displayName.localeCompare(b.displayName, undefined, {
      sensitivity: 'base'
    })
    if (byName !== 0) return byName
    return (originalIndex.get(a.id) ?? 0) - (originalIndex.get(b.id) ?? 0)
  })
}
