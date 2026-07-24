import type { Repo } from '../../../shared/types'

/** Exact-path lookup over the repo registry that tolerates spec-015 dual
 *  entries (same path, local + remote): prefers the local entry
 *  (no connectionId), else the first match — deterministic regardless of
 *  registry reorder. */
export function findRepoByPathPreferLocal(
  repos: Repo[] | undefined,
  path: string
): Repo | undefined {
  let firstMatch: Repo | undefined
  for (const repo of repos ?? []) {
    if (repo.path !== path) {
      continue
    }
    if (repo.connectionId == null) {
      return repo
    }
    firstMatch ??= repo
  }
  return firstMatch
}
