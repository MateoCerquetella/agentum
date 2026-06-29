// Typed client for the conversational chat route on the embedded agentum-server
// (`POST /api/chat`). Mirrors `board-client.ts`: same loopback endpoint + bearer
// auth (`getServerEndpoint` → `apiUrl`), wire shapes faithful to the server's
// `/api/chat` contract. The endpoint is a Socratic interviewer — it asks
// clarifying questions, then proposes a task breakdown the user can file into
// GitHub or Linear. This client is conversation-only; it never creates tasks.
import { apiUrl, getServerEndpoint } from './server-endpoint'

/** One conversation turn — matches the server's `{role, content}` message shape. */
export type ChatTurn = { role: 'user' | 'assistant'; content: string }

/** A model offered in the Chat model picker. `id` is the Anthropic model id sent
 *  to `/api/chat/stream`; `label`/`blurb` are display-only. */
export type ChatModel = { id: string; label: string; blurb: string }

/** The models the Chat picker offers. All current Claude models support extended
 *  thinking, so the thinking toggle applies to any of them. Default = Sonnet, the
 *  server's own default (`DEFAULT_MODEL` in `routes/chat.rs`). */
export const CHAT_MODELS: readonly ChatModel[] = [
  { id: 'claude-opus-4-8', label: 'Claude Opus 4.8', blurb: 'Most capable' },
  { id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6', blurb: 'Balanced · default' },
  { id: 'claude-haiku-4-5-20251001', label: 'Claude Haiku 4.5', blurb: 'Fastest' }
] as const

export const DEFAULT_CHAT_MODEL = 'claude-sonnet-4-6'

/** Resolve a stored/unknown model id to a known {@link ChatModel}, falling back
 *  to the default so a removed model never strands the picker on a blank label. */
export function resolveChatModel(id: string | null | undefined): ChatModel {
  return CHAT_MODELS.find((m) => m.id === id) ?? CHAT_MODELS.find((m) => m.id === DEFAULT_CHAT_MODEL) ?? CHAT_MODELS[0]
}

/** One delta from `/api/chat/stream` — mirrors the server's compact SSE events. */
export type ChatStreamDelta =
  | { type: 'text'; text: string }
  | { type: 'thinking'; text: string }
  | { type: 'error'; message: string }
  | { type: 'done' }

async function authHeaders(): Promise<Record<string, string>> {
  const { token } = await getServerEndpoint()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

/**
 * `POST /api/chat` — send the running transcript and get the assistant's next
 * reply. Mirrors board-client's `createGoal`: own fetch path so the typed error
 * envelope survives. On a non-ok response, surfaces the server's message text:
 * handles BOTH `{ error: string }` (400) and `{ error: { message } }` (502
 * `llm_failed`) shapes, throwing `Error(<server message>)` either way.
 */
export async function sendChat(
  messages: ChatTurn[],
  opts?: { workdir?: string; repoSlug?: string }
): Promise<string> {
  const url = await apiUrl('/api/chat')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders())
    },
    body: JSON.stringify({
      messages,
      workdir: opts?.workdir,
      repo_slug: opts?.repoSlug
    })
  })
  const text = await res.text()
  if (res.ok) {
    const parsed = (text ? JSON.parse(text) : {}) as { content?: string }
    return parsed.content ?? ''
  }
  // Surface the specific reason. The server uses two error shapes:
  //   400 → { error: "Sign in to Claude…" }   (string)
  //   502 → { error: { code, message } }       (object, e.g. llm_failed)
  let message = `chat ${res.status}`
  try {
    const parsed = JSON.parse(text) as {
      error?: { message?: string } | string
    }
    if (parsed.error && typeof parsed.error === 'object') {
      message = parsed.error.message ?? message
    } else if (typeof parsed.error === 'string') {
      message = parsed.error
    }
  } catch {
    if (text) message = text
  }
  throw new Error(message)
}

/**
 * `POST /api/chat/stream` — stream the assistant's next reply token-by-token.
 * Invokes `onDelta` for each `text`/`thinking` chunk as it arrives and resolves
 * with the fully assembled `{ content, thinking }` when the stream ends. The
 * server sends compact one-line SSE `data:` events (`text` | `thinking` | `error`
 * | `done`); we frame on the blank-line separator and JSON-parse each `data:`.
 *
 * Errors surface the same way as {@link sendChat}: an upstream non-2xx (no stream
 * opened) throws the typed server message, and a mid-stream `error` event throws
 * its message. Pass `opts.signal` to abort (the rejection is the caller's to
 * swallow). Whatever streamed before the failure is preserved via `onDelta`.
 */
export async function streamChat(
  messages: ChatTurn[],
  opts: {
    workdir?: string
    repoSlug?: string
    model?: string
    thinking?: boolean
    signal?: AbortSignal
    onDelta?: (delta: ChatStreamDelta) => void
  } = {}
): Promise<{ content: string; thinking: string }> {
  const url = await apiUrl('/api/chat/stream')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders())
    },
    body: JSON.stringify({
      // Send only the wire shape; any UI-only fields (e.g. stored `thinking`) are
      // dropped so the server gets a clean `{role, content}[]`.
      messages: messages.map((m) => ({ role: m.role, content: m.content })),
      workdir: opts.workdir,
      repo_slug: opts.repoSlug,
      model: opts.model,
      thinking: opts.thinking ?? false
    }),
    signal: opts.signal
  })

  // The stream never opened — decode the typed error envelope (string or object).
  if (!res.ok || !res.body) {
    const text = await res.text().catch(() => '')
    let message = `chat ${res.status}`
    try {
      const parsed = JSON.parse(text) as { error?: { message?: string } | string }
      if (parsed.error && typeof parsed.error === 'object') message = parsed.error.message ?? message
      else if (typeof parsed.error === 'string') message = parsed.error
    } catch {
      if (text) message = text
    }
    throw new Error(message)
  }

  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buf = ''
  let content = ''
  let thinking = ''

  // Apply one parsed SSE event. Throws on `error` so the caller can surface it
  // (partial `content`/`thinking` already delivered via onDelta is preserved).
  const apply = (data: string): void => {
    let ev: ChatStreamDelta
    try {
      ev = JSON.parse(data) as ChatStreamDelta
    } catch {
      return
    }
    if (ev.type === 'text') {
      content += ev.text
      opts.onDelta?.(ev)
    } else if (ev.type === 'thinking') {
      thinking += ev.text
      opts.onDelta?.(ev)
    } else if (ev.type === 'error') {
      throw new Error(ev.message || 'the model stream errored')
    }
    // `done` is implied by stream end; nothing to accumulate.
  }

  try {
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      buf += decoder.decode(value, { stream: true })
      // SSE frames are separated by a blank line; each carries one `data:` JSON.
      let idx = buf.indexOf('\n\n')
      while (idx !== -1) {
        const frame = buf.slice(0, idx)
        buf = buf.slice(idx + 2)
        for (const line of frame.split('\n')) {
          const trimmed = line.replace(/^\s+/, '')
          if (!trimmed.startsWith('data:')) continue
          const data = trimmed.slice('data:'.length).trim()
          if (data) apply(data)
        }
        idx = buf.indexOf('\n\n')
      }
    }
  } finally {
    try {
      reader.releaseLock()
    } catch {
      // Lock may already be released after an abort/error — nothing to do.
    }
  }

  return { content, thinking }
}

/** Which tracker the Chat files into. GitHub/Linear only (never the board). */
export type IssueProvider = 'github' | 'linear'

/** One created issue. `url` is the issue link; `id` is the tracker's stable
 *  handle (a GitHub issue number is implicit in the url; Linear sends its
 *  human identifier like `ENG-42`). */
export type CreatedIssue = { title: string; url: string; id?: string }

/** Result of `POST /api/chat/issues` — what landed on the tracker, and what
 *  didn't. `repo` is set on the GitHub path only. */
export type CreatedIssues = {
  provider: IssueProvider
  repo?: string
  created: CreatedIssue[]
  failed: { title: string; error: string }[]
}

/**
 * Coerce user-typed repo input into the `owner/repo` slug the server expects.
 * Accepts a bare `owner/repo`, a full GitHub URL (`https://github.com/owner/repo`,
 * optionally `.git`), or an SSH remote (`git@github.com:owner/repo.git`). Returns
 * '' when nothing usable is found — the caller treats '' as "omit `repo_slug` and
 * fall back to the open project's origin". This is what lets the Chat file issues
 * with no local project connected: type the repo, and it goes straight through.
 */
export function normalizeRepoSlug(raw: string | null | undefined): string {
  const s = (raw ?? '').trim()
  if (!s) return ''
  // owner/repo embedded in an https or ssh GitHub remote.
  const m = s.match(/github\.com[/:]([^/\s]+)\/([^/\s]+?)(?:\.git)?\/?$/i)
  if (m) return `${m[1]}/${m[2]}`
  // Bare `owner/repo` (tolerate a trailing .git or slash).
  return s.replace(/\/+$/, '').replace(/\.git$/i, '')
}

/**
 * `POST /api/chat/issues` — distil the agreed task breakdown from the transcript
 * and file one issue per task into the chosen tracker (`provider`, default
 * `github`). Partial success is a 200 (`created` + `failed` per-task). Mirrors
 * `sendChat`'s fetch/error handling: a non-ok response throws
 * `Error(<server message>)`, decoding BOTH the `{ error: string }` (400/422
 * string envelopes — e.g. an unknown provider) and `{ error: { message } }`
 * (502 `llm_failed`, 422 `no_tasks`/`no_github_repo`/`no_linear`) shapes.
 */
export async function createIssuesFromChat(
  messages: ChatTurn[],
  opts?: { workdir?: string; repoSlug?: string; provider?: IssueProvider }
): Promise<CreatedIssues> {
  const url = await apiUrl('/api/chat/issues')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders())
    },
    body: JSON.stringify({
      messages,
      workdir: opts?.workdir,
      // Normalize a typed repo (URL/SSH/bare) to `owner/repo`; '' → omit so the
      // server falls back to the open project's origin.
      repo_slug: normalizeRepoSlug(opts?.repoSlug) || undefined,
      provider: opts?.provider
    })
  })
  const text = await res.text()
  if (res.ok) {
    const parsed = (text ? JSON.parse(text) : {}) as Partial<CreatedIssues>
    return {
      provider: parsed.provider ?? opts?.provider ?? 'github',
      repo: parsed.repo,
      created: parsed.created ?? [],
      failed: parsed.failed ?? []
    }
  }
  let message = `chat issues ${res.status}`
  try {
    const parsed = JSON.parse(text) as { error?: { message?: string } | string }
    if (parsed.error && typeof parsed.error === 'object') {
      message = parsed.error.message ?? message
    } else if (typeof parsed.error === 'string') {
      message = parsed.error
    }
  } catch {
    if (text) message = text
  }
  throw new Error(message)
}
