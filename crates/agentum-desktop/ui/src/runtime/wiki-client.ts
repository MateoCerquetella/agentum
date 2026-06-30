// Typed client for the AutoWiki routes on the embedded agentum-server
// (`/api/wiki*`, spec 001). Mirrors `harness-client.ts`: thin calls over the
// shared loopback HTTP helpers in `server-http.ts`. The wire shapes here are
// kept faithful to `crates/agentum-server/src/routes/wiki.rs` (serde camelCase,
// internally tagged on `state`) so there is one source of truth and no silent
// field drift.
import { getJson, postJson, qs } from './server-http'

/** One TOC entry — matches the Rust `WikiPageMeta`. `slug` is the page filename
 *  stem + URL segment; `title` is the human label and the `[[Title]]` target. */
export type WikiPageMeta = {
  slug: string
  title: string
}

/**
 * The browse-time state of a workdir's wiki — the `GET /api/wiki` response,
 * discriminated on `state` (the Rust enum is internally tagged on `state`).
 */
export type WikiIndexResponse =
  | { state: 'empty' }
  | { state: 'running'; sessionId: string }
  | { state: 'failed'; error: string }
  | { state: 'ready'; schemaVersion: number; generatedAt: number; pages: WikiPageMeta[] }

/** `GET /api/wiki?workdir=` — the TOC / generation state for a workdir. */
export function getWiki(workdir: string): Promise<WikiIndexResponse> {
  return getJson<WikiIndexResponse>(`/api/wiki${qs({ workdir })}`)
}

/** `GET /api/wiki/{slug}?workdir=` — one page's markdown (JSON `{ content }`). */
export function getWikiPage(workdir: string, slug: string): Promise<{ content: string }> {
  return getJson<{ content: string }>(`/api/wiki/${encodeURIComponent(slug)}${qs({ workdir })}`)
}

/** `POST /api/wiki/generate` — spawn the agent that writes the wiki; returns the
 *  job's session id so the run is observable like any other session. */
export function generateWiki(workdir: string): Promise<{ sessionId: string }> {
  return postJson<{ sessionId: string }>('/api/wiki/generate', { workdir })
}
