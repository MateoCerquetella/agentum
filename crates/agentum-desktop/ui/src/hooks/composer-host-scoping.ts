// Pure host-scoping helpers for the New Workspace composer (spec 006).
// `useComposerState` owns the React state; this module holds the derivations so
// the host→repo scoping + default-host + repoId-reset rules are unit-testable
// without mounting the hook (mirrors composer-branch-selection.ts).
import type { Repo } from '@/shared/types'
import type { HostKey, HostMeta } from '@/store/slices/hosts'
import { LOCAL_HOST_KEY, hostKeyForRepo } from '@/components/sidebar/worktree-list-groups'



/** A host option in the composer's host selector. */
export type ComposerHostOption = {
  key: HostKey
  kind: 'local' | 'ssh'
  label: string
}

/** Distinct hosts: local + every known SSH host from `hostMetaByKey`, then any
 *  additional hosts referenced by repos. SSH hosts always appear (even without
 *  repos) so the composer host selector works on fresh installs after adding an
 *  SSH connection. Labels come from `hostMetaByKey`; a host with no meta yet
 *  (readiness still hydrating) falls back to a kind-derived placeholder so the
 *  selector renders before `hydrateHosts` lands. */
export function deriveEligibleHosts(
  eligibleRepos: Repo[],
  hostMetaByKey: Record<HostKey, HostMeta>
): ComposerHostOption[] {
  const seen = new Set<HostKey>()
  const order: HostKey[] = []

  // Always include local first.
  seen.add(LOCAL_HOST_KEY)
  order.push(LOCAL_HOST_KEY)

  // Include every known SSH host (even those without repos yet).
  for (const key of Object.keys(hostMetaByKey)) {
    if (key !== LOCAL_HOST_KEY && !seen.has(key)) {
      seen.add(key)
      order.push(key)
    }
  }

  // Include any additional hosts referenced by repos (catches hosts not yet
  // hydrated into hostMetaByKey).
  for (const repo of eligibleRepos) {
    const key = hostKeyForRepo(repo)
    if (!seen.has(key)) {
      seen.add(key)
      order.push(key)
    }
  }

  return order.map((key) => {
    const meta = hostMetaByKey[key]
    const kind: 'local' | 'ssh' = key === LOCAL_HOST_KEY ? 'local' : 'ssh'
    return {
      key,
      kind,
      label: meta?.label ?? (kind === 'local' ? 'This machine' : 'SSH host')
    }
  })
}

/** Repos on a single host. Local = repos with no `connectionId`. */
export function filterReposForHost(eligibleRepos: Repo[], hostKey: HostKey): Repo[] {
  return eligibleRepos.filter((repo) => hostKeyForRepo(repo) === hostKey)
}

/** The host the composer should open on: the active repo's host when that repo
 *  is still eligible, else the first eligible host, else local. (PM-resolved:
 *  active host else local.) */
export function resolveDefaultHostKey(
  eligibleRepos: Repo[],
  activeRepoId: string | null,
  eligibleHosts: ComposerHostOption[]
): HostKey {
  if (activeRepoId) {
    const activeRepo = eligibleRepos.find((repo) => repo.id === activeRepoId)
    if (activeRepo) {
      return hostKeyForRepo(activeRepo)
    }
  }
  return eligibleHosts[0]?.key ?? LOCAL_HOST_KEY
}

/** The repoId to select after a host switch (or on open). Keeps the current
 *  selection only when it belongs to the host's repos; otherwise resets to the
 *  host's first repo (empty string when the host has none). */
export function resolveRepoIdForHost(hostScopedRepos: Repo[], currentRepoId: string): string {
  if (currentRepoId && hostScopedRepos.some((repo) => repo.id === currentRepoId)) {
    return currentRepoId
  }
  return hostScopedRepos[0]?.id ?? ''
}

/** Per-`(hostKey, repoId)` cache key for the `worktrees/detected` authoritative
 *  flag — a repo's git-ness is host-specific, so a bare repoId would alias the
 *  same repo across hosts. */
export function gitOnHostCacheKey(hostKey: HostKey, repoId: string): string {
  return `${hostKey}::${repoId}`
}
