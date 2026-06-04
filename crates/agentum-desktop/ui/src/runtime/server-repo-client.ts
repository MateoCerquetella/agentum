// Repo registry / base-ref client over the embedded agentum-server
// (`/api/repos/*`). The repo logic moved off the desktop's native commands into
// `agentum-server/src/routes/repos.rs`; this is the typed boundary the UI calls.
import type { BaseRefSearchResult, Repo } from '../../../shared/types'
import { del, getJson, patchJson, postJson, qs } from './server-http'

/** `GET /api/repos` — the registered repos, in order. */
export function reposList(): Promise<Repo[]> {
  return getJson<Repo[]>('/api/repos')
}

/** `POST /api/repos` — register a path. Returns `{repo}` or `{error}`. */
export function reposAdd(path: string, kind?: string): Promise<{ repo?: Repo; error?: string }> {
  return postJson('/api/repos', { path, ...(kind ? { kind } : {}) })
}

/** `PATCH /api/repos/{id}` — apply `updates` (id/path/addedAt are ignored). */
export function reposUpdate(repoId: string, updates: Record<string, unknown>): Promise<Repo> {
  return patchJson<Repo>(`/api/repos/${encodeURIComponent(repoId)}`, updates)
}

/** `POST /api/repos/create` — make a folder (optionally `git init`) + register. */
export function reposCreate(args: {
  parentPath: string
  name: string
  kind: string
}): Promise<{ repo?: Repo; error?: string }> {
  return postJson('/api/repos/create', args)
}

/** `POST /api/repos/clone` — `git clone` + register. */
export function reposClone(url: string, destination: string): Promise<Repo> {
  return postJson<Repo>('/api/repos/clone', { url, destination })
}

/** `DELETE /api/repos/{id}` — drop from the registry. */
export function reposRemove(repoId: string): Promise<void> {
  return del(`/api/repos/${encodeURIComponent(repoId)}`)
}

/** `POST /api/repos/reorder` — apply an explicit order. */
export function reposReorder(orderedIds: string[]): Promise<{ status: string }> {
  return postJson('/api/repos/reorder', { orderedIds })
}

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
