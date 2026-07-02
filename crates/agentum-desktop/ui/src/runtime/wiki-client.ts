// Typed client for the AutoWiki routes on the embedded agentum-server
// (`/api/wiki*`, spec 001). Mirrors `harness-client.ts`: thin calls over the
// shared loopback HTTP helpers in `server-http.ts`. The wire shapes here are
// kept faithful to `crates/agentum-server/src/routes/wiki.rs` (serde camelCase,
// internally tagged on `state`) so there is one source of truth and no silent
// field drift.
//
// Routes are keyed by `repoId` (not a raw path): the server resolves the repo's
// HOST + git remote and stores the wiki centrally keyed by that git identity, so
// a repo cloned locally AND over SSH shares ONE wiki instead of duplicating it
// per checkout path.
import { getJson, postJson, qs } from './server-http'

/** One TOC entry — matches the Rust `WikiPageMeta`. `slug` is the page filename
 *  stem + URL segment; `title` is the human label and the `[[Title]]` target. */
export type WikiPageMeta = {
  slug: string
  title: string
}

/**
 * The browse-time state of a project's wiki — the `GET /api/wiki` response,
 * discriminated on `state` (the Rust enum is internally tagged on `state`).
 */
export type WikiIndexResponse =
  | { state: 'empty' }
  | { state: 'running'; sessionId: string }
  | { state: 'failed'; error: string }
  | { state: 'ready'; schemaVersion: number; generatedAt: number; pages: WikiPageMeta[] }

/** `GET /api/wiki?repoId=` — the TOC / generation state for a project. */
export function getWiki(repoId: string): Promise<WikiIndexResponse> {
  return getJson<WikiIndexResponse>(`/api/wiki${qs({ repoId })}`)
}

/** `GET /api/wiki/{slug}?repoId=` — one page's markdown (JSON `{ content }`). */
export function getWikiPage(repoId: string, slug: string): Promise<{ content: string }> {
  return getJson<{ content: string }>(`/api/wiki/${encodeURIComponent(slug)}${qs({ repoId })}`)
}

/** Which agent + model generate the wiki. Both optional — omit for the Claude
 *  default (preserves the prior behaviour). `model` is a hint passed to the
 *  agent's `--model` (e.g. `claude-opus-4-8`). */
export type WikiGenerateOptions = {
  tool?: string
  model?: string
}

/** `POST /api/wiki/generate` — spawn the agent that writes the wiki; returns the
 *  job's session id so the run is observable like any other session. */
export function generateWiki(
  repoId: string,
  opts?: WikiGenerateOptions
): Promise<{ sessionId: string }> {
  return postJson<{ sessionId: string }>('/api/wiki/generate', {
    repoId,
    tool: opts?.tool,
    model: opts?.model
  })
}

/** `POST /api/wiki/export` — write a committable copy of the wiki into the repo
 *  (`<repo>/.agentum/wiki`) and un-gitignore it, so the user can commit it. */
export function exportWikiToRepo(repoId: string): Promise<{ path: string; files: number }> {
  return postJson<{ path: string; files: number }>('/api/wiki/export', { repoId })
}
