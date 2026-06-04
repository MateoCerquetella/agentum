// Repo registry / base-ref client over the embedded agentum-server
// (`/api/repos/*`). The repo logic moved off the desktop's native commands into
// `agentum-server/src/routes/repos.rs`; this is the typed boundary the UI calls.
import type { BaseRefSearchResult } from '../../../shared/types'
import { getJson, qs } from './server-http'

export type ServerRepoBaseRefDefault = {
  defaultBaseRef: string | null
  remoteCount: number
}

/** origin's default head / local main|master / current branch, + remote count. */
export function getServerRepoBaseRefDefault(repoId: string): Promise<ServerRepoBaseRefDefault> {
  return getJson<ServerRepoBaseRefDefault>(
    `/api/repos/${encodeURIComponent(repoId)}/base-ref-default`
  )
}

/** Matching ref names (local + remote), capped at `limit`. */
export function getServerRepoBaseRefs(
  repoId: string,
  query: string,
  limit: number
): Promise<string[]> {
  return getJson<string[]>(
    `/api/repos/${encodeURIComponent(repoId)}/base-refs${qs({ q: query, limit })}`
  )
}

/** Matching refs as `{refName, localBranchName}` pairs, capped at `limit`. */
export function getServerRepoBaseRefDetails(
  repoId: string,
  query: string,
  limit: number
): Promise<BaseRefSearchResult[]> {
  return getJson<BaseRefSearchResult[]>(
    `/api/repos/${encodeURIComponent(repoId)}/base-ref-details${qs({ q: query, limit })}`
  )
}
