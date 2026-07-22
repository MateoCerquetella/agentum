// Typed client for `GET /api/github/issue` on the embedded agentum-server.
// Mirrors `board-client.ts`: same loopback endpoint + bearer auth. The Tasks
// "Use" path calls this to fetch a GitHub issue's body so the spawned agent
// starts from the spec — the same way Linear already snapshots its description
// into linked context (spec 002, Option B). Wire shape is faithful to
// `crates/agentum-server/src/routes/github.rs::IssueBody`.
import { apiUrl, getServerEndpoint } from './server-endpoint'
import type { ChatAgentId } from './chat-client'

export type DraftLlmChoice = {
  agent: ChatAgentId
  model?: string
}

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
  /** Spec 020 F3: resolve the slug (and run `gh`) on this repo's own host. */
  repoId?: string
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
  if (input.repoId) {
    params.set('repoId', input.repoId)
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

/**
 * Fetch the repo's existing GitHub label names via `gh label list` on the
 * embedded server (spec 006 F1, D2) — seeds the composer's label picker.
 * Throws on any non-2xx (or timeout); the caller falls back to the static
 * `type/*`+`priority/*` set, so a label fetch must never block filing.
 */
export async function fetchGithubRepoLabels(input: {
  workdir: string
  slug?: string
  /** Resolve the slug on the registered repo's host (required for SSH paths). */
  repoId?: string
  /** Abort budget — a slow/hung `gh` must not delay the composer. */
  timeoutMs?: number
}): Promise<string[]> {
  const params = new URLSearchParams({ workdir: input.workdir })
  if (input.slug) {
    params.set('slug', input.slug)
  }
  if (input.repoId) {
    params.set('repoId', input.repoId)
  }
  const url = await apiUrl(`/api/github/labels?${params.toString()}`)

  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 6000)
  try {
    const res = await fetch(url, {
      headers: { ...(await authHeaders()) },
      signal: controller.signal
    })
    if (!res.ok) {
      const detail = await res.text().catch(() => '')
      throw new Error(`github labels ${res.status}${detail ? ` — ${detail}` : ''}`)
    }
    const text = await res.text()
    return (JSON.parse(text) as { labels: string[] }).labels
  } finally {
    window.clearTimeout(timeout)
  }
}

// Wire shape of `POST /api/github/issues`
// (crates/agentum-server/src/routes/github.rs::CreateIssueResponse).
export type CreatedGithubIssue = {
  provider: 'github'
  number: number
  url: string
  slug: string
  /** Spec 006 F4: the authenticated `gh` login — null when the best-effort
   *  lookup failed (the issue itself was still created). */
  author: string | null
}

/** The create-issue request body (spec 020 F3). Pure + exported for the wire
 *  pins: absent optionals produce absent keys, so a repoId-less call keeps the
 *  pre-020 body byte-identical. */
export function createIssuePayload(input: {
  title: string
  body?: string
  workdir: string
  slug?: string
  repoId?: string
  labels?: string[]
}): Record<string, unknown> {
  return {
    title: input.title,
    ...(input.body ? { body: input.body } : {}),
    workdir: input.workdir,
    ...(input.slug ? { slug: input.slug } : {}),
    ...(input.repoId ? { repoId: input.repoId } : {}),
    // Omitted when empty so the pre-006 wire shape stays byte-identical.
    ...(input.labels?.length ? { labels: input.labels } : {})
  }
}

/**
 * File a new GitHub issue through the embedded server's `TaskSink::Github`
 * path (spec 004 F3) — the composer's issue-first affordance. `workdir` is the
 * selected repo's path (used for the `origin` slug read when no `slug` hint is
 * supplied); `repoId` (spec 020 F3) makes that read run on the repo's own
 * host — the robustness path when no slug was learned. Throws on any non-2xx
 * so the caller can render an inline error without mutating composer state.
 */
export async function createGithubIssue(input: {
  title: string
  body?: string
  workdir: string
  slug?: string
  repoId?: string
  /** Spec 006 F1: labels applied at creation (existing repo label names). */
  labels?: string[]
  /** Abort budget — issue creation shells out to `gh`. */
  timeoutMs?: number
}): Promise<CreatedGithubIssue> {
  const url = await apiUrl('/api/github/issues')
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 15000)
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
      body: JSON.stringify(createIssuePayload(input)),
      signal: controller.signal
    })
    if (!res.ok) {
      const detail = await res.text().catch(() => '')
      throw new Error(`create issue ${res.status}${detail ? ` — ${detail}` : ''}`)
    }
    const text = await res.text()
    return JSON.parse(text) as CreatedGithubIssue
  } finally {
    window.clearTimeout(timeout)
  }
}

/** Pull a human-readable message out of an agentum-server error body — either
 *  the typed `{ error: { message } }` envelope or a plain-text body. Exported
 *  for tests. */
export function extractServerErrorMessage(raw: string, fallback: string): string {
  const trimmed = raw.trim()
  if (!trimmed) {
    return fallback
  }
  try {
    const parsed = JSON.parse(trimmed) as { error?: { message?: string } | string }
    if (typeof parsed.error === 'string' && parsed.error.trim()) {
      return parsed.error
    }
    if (parsed.error && typeof parsed.error === 'object' && parsed.error.message) {
      return parsed.error.message
    }
  } catch {
    // Not JSON — the body itself is the message (ApiError::BadRequest is text).
  }
  return trimmed
}

// Wire shape of `POST /api/github/issues/draft-body`
// (crates/agentum-server/src/routes/github.rs::DraftBodyResponse).
export type DraftedGithubIssueBody = {
  body: string
  /** Spec 020 F3 (D4): whether repo/wiki context actually grounded the draft.
   *  Optional client-side only to tolerate an older-server skew — the embedded
   *  server ships lockstep, so it is effectively always present. */
  grounding?: { repo: boolean; wiki: boolean }
}

/**
 * Draft an SDD-shaped issue body (## Problem / ## Goal / ## Acceptance
 * criteria checklist) from the typed title + local repo context (spec 007).
 * The composer puts the result in the body TEXTAREA for review — this call
 * never files anything. Throws with the server's message on any non-2xx so
 * the form can render it inline (including the "set ANTHROPIC_API_KEY / sign
 * in to Claude" no-credentials message).
 */
export function draftIssueBodyPayload(input: {
  workdir: string
  title: string
  slug?: string
  agent?: ChatAgentId
  model?: string
}): Record<string, string> {
  return {
    workdir: input.workdir,
    title: input.title,
    ...(input.slug ? { slug: input.slug } : {}),
    ...(input.agent ? { agent: input.agent } : {}),
    ...(input.model ? { model: input.model } : {})
  }
}

export async function draftGithubIssueBody(input: {
  workdir: string
  title: string
  slug?: string
  agent?: ChatAgentId
  model?: string
  /** Abort budget — a full LLM draft; generous but bounded. */
  timeoutMs?: number
}): Promise<DraftedGithubIssueBody> {
  const url = await apiUrl('/api/github/issues/draft-body')
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 60000)
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
      body: JSON.stringify(draftIssueBodyPayload(input)),
      signal: controller.signal
    })
    if (!res.ok) {
      const detail = await res.text().catch(() => '')
      throw new Error(
        extractServerErrorMessage(detail, `draft description failed (${res.status})`)
      )
    }
    const text = await res.text()
    return JSON.parse(text) as DraftedGithubIssueBody
  } finally {
    window.clearTimeout(timeout)
  }
}

// Wire shape of `POST /api/harness/spec-from-issue`
// (crates/agentum-server/src/routes/harness.rs::SpecFromIssueResponse).
export type ScaffoldedSpecFromIssue = {
  specId: string
  specExisted: boolean
  specPath: string
  written: string[]
}

/**
 * Scaffold `.agentum-harness/specs/<n>-<slug>/spec.md` (+ a tracker-stamped
 * backlog) from a linked GitHub issue into a freshly created worktree
 * (spec 004 F4, opt-in via the composer's "Scaffold spec" toggle — D5).
 * Failures are the caller's to swallow: the workspace must stay usable even
 * when the scaffold fails.
 */
export async function scaffoldSpecFromIssue(input: {
  /** The new worktree's absolute path — the spec is written INTO it. */
  workdir: string
  number: number
  slug?: string
  /** Also derive feature_list.json (server default: true). */
  plan?: boolean
  /** Retain an existing human-edited spec and return success (retry/adoption). */
  converge?: boolean
  /** Spec 021 (#379): the repo's tracker pin (`auto`/`github`/`linear`).
   *  Absent/`auto` keeps the issue-driven path's GitHub stamping. */
  tracker?: string
  timeoutMs?: number
}): Promise<ScaffoldedSpecFromIssue> {
  const url = await apiUrl('/api/harness/spec-from-issue')
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), input.timeoutMs ?? 15000)
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...(await authHeaders()) },
      body: JSON.stringify({
        workdir: input.workdir,
        number: String(input.number),
        ...(input.slug ? { slug: input.slug } : {}),
        ...(input.plan !== undefined ? { plan: input.plan } : {}),
        ...(input.converge !== undefined ? { converge: input.converge } : {}),
        ...(input.tracker ? { tracker: input.tracker } : {})
      }),
      signal: controller.signal
    })
    if (!res.ok) {
      const detail = await res.text().catch(() => '')
      throw new Error(`spec from issue ${res.status}${detail ? ` — ${detail}` : ''}`)
    }
    const text = await res.text()
    return JSON.parse(text) as ScaffoldedSpecFromIssue
  } finally {
    window.clearTimeout(timeout)
  }
}
