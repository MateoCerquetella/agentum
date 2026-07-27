import { api } from '@/tauri'
import type { BaseRefSearchResult, GlobalSettings } from '@/shared/types'
import { legacyBaseRefSearchResult } from '@/shared/base-ref-search-result'
import { callRuntimeRpc, getActiveRuntimeTarget } from './runtime-rpc-client'
import {
  getServerRepoBaseRefDefault,
  getServerRepoBaseRefs,
  getServerRepoBaseRefDetails
} from './server-repo-client'

export type RuntimeRepoBaseRefDefault = {
  defaultBaseRef: string | null
  remoteCount: number
}

/**
 * Run a server-backed repo READ, falling back to the native `local()` if it
 * throws — a server hiccup degrades to the proven local path instead of breaking
 * the base-ref picker. Reads are idempotent, so retrying locally is safe. (The
 * registry CRUD still uses the native commands until that move lands.)
 */
async function serverRepoRead<T>(server: () => Promise<T>, local: () => Promise<T>): Promise<T> {
  try {
    return await server()
  } catch (error) {
    console.warn('[agentum] server repo read failed, using local:', error)
    return local()
  }
}

export async function getRuntimeRepoBaseRefDefault(
  settings: Pick<GlobalSettings, 'activeRuntimeEnvironmentId'> | null | undefined,
  repoId: string
): Promise<RuntimeRepoBaseRefDefault> {
  const target = getActiveRuntimeTarget(settings)
  if (target.kind !== 'environment') {
    return serverRepoRead(
      () => getServerRepoBaseRefDefault(repoId),
      () => api.repos.getBaseRefDefault({ repoId })
    )
  }
  return callRuntimeRpc<RuntimeRepoBaseRefDefault>(
    target,
    'repo.baseRefDefault',
    { repo: repoId },
    { timeoutMs: 15_000 }
  )
}

export async function searchRuntimeRepoBaseRefs(
  settings: Pick<GlobalSettings, 'activeRuntimeEnvironmentId'> | null | undefined,
  repoId: string,
  query: string,
  limit: number
): Promise<string[]> {
  const target = getActiveRuntimeTarget(settings)
  if (target.kind !== 'environment') {
    return serverRepoRead(
      () => getServerRepoBaseRefs(repoId, query, limit),
      () => api.repos.searchBaseRefs({ repoId, query, limit })
    )
  }
  const result = await callRuntimeRpc<{ refs: string[]; truncated: boolean }>(
    target,
    'repo.searchRefs',
    { repo: repoId, query, limit },
    { timeoutMs: 15_000 }
  )
  return result.refs
}

export async function searchRuntimeRepoBaseRefDetails(
  settings: Pick<GlobalSettings, 'activeRuntimeEnvironmentId'> | null | undefined,
  repoId: string,
  query: string,
  limit: number
): Promise<BaseRefSearchResult[]> {
  const target = getActiveRuntimeTarget(settings)
  if (target.kind !== 'environment') {
    return serverRepoRead(
      () => getServerRepoBaseRefDetails(repoId, query, limit),
      () => api.repos.searchBaseRefDetails({ repoId, query, limit })
    )
  }
  const result = await callRuntimeRpc<{
    refs: string[]
    refDetails?: BaseRefSearchResult[]
    truncated: boolean
  }>(target, 'repo.searchRefs', { repo: repoId, query, limit }, { timeoutMs: 15_000 })
  return result.refDetails ?? result.refs.map(legacyBaseRefSearchResult)
}
