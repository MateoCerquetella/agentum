// Typed client for `GET /api/github/issue` on the embedded agentum-server.
// Mirrors `board-client.ts`: same loopback endpoint + bearer auth. The Tasks
// "Use" path calls this to fetch a GitHub issue's body so the spawned agent
// starts from the spec — the same way Linear already snapshots its description
// into linked context (spec 002, Option B). Wire shape is faithful to
// `crates/agentum-server/src/routes/github.rs::IssueBody`.
import { apiUrl, getServerEndpoint } from './server-endpoint'

export type GithubIssueBody = {
  title: string
  body: string
}

async function authHeaders(): Promise<Record<string, string>> {
  const { token } = await getServerEndpoint()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

/**
 * Fetch a single GitHub issue's title + body via the embedded server's `gh`
 * runner. `slug` (owner/repo, parsed from the item URL) lets the server skip the
 * `origin` read; `workdir` is the project dir used as the resolution fallback.
 * Throws on any non-2xx (or timeout) so the caller can fall back to the
 * title+URL prompt — fetching the body must never break "Use".
 */
export async function fetchGithubIssueBody(input: {
  number: number
  workdir: string
  slug?: string
  /** Abort budget — a slow/hung `gh` must not delay the composer. */
  timeoutMs?: number
}): Promise<GithubIssueBody> {
  const params = new URLSearchParams({
    number: String(input.number),
    workdir: input.workdir
  })
  if (input.slug) {
    params.set('slug', input.slug)
  }
  const url = await apiUrl(`/api/github/issue?${params.toString()}`)

  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 6000)
  try {
    const res = await fetch(url, {
      headers: { ...(await authHeaders()) },
      signal: controller.signal
    })
    if (!res.ok) {
      const detail = await res.text().catch(() => '')
      throw new Error(`github issue ${res.status}${detail ? ` — ${detail}` : ''}`)
    }
    const text = await res.text()
    return JSON.parse(text) as GithubIssueBody
  } finally {
    window.clearTimeout(timeout)
  }
}
