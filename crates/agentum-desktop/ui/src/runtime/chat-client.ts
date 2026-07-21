// Typed client for the conversational chat route on the embedded agentum-server
// (`POST /api/chat`). Mirrors `board-client.ts`: same loopback endpoint + bearer
// auth (`getServerEndpoint` → `apiUrl`), wire shapes faithful to the server's
// `/api/chat` contract. The endpoint is a Socratic interviewer — it asks
// clarifying questions, then proposes a task breakdown the user can file into
// GitHub or Linear. This client is conversation-only; it never creates tasks.
import { buildChatBody, buildChatStreamBody } from '../lib/chat-body'
import type { IntakeMode } from '../lib/socratic-intake'
import type { ChatAgentId, TuiAgent } from '../shared/types'
import { apiUrl, getServerEndpoint } from './server-endpoint'

export type { ChatAgentId }

/** One conversation turn — matches the server's `{role, content}` message shape. */
export type ChatTurn = { role: 'user' | 'assistant'; content: string }

/** A chat agent the user can pick in Settings (spec 394). `label` is the
 *  visible name; `blurb` names where its credentials come from. The server has
 *  a backend for exactly these ids (`ChatAgent` in `routes/chat_agent.rs`). */
export type ChatAgent = { id: ChatAgentId; label: string; blurb: string }

/** The agents the Settings → Chat picker offers. Order = display order; the
 *  first entry is the default (Claude — the pre-setting behavior). */
export const CHAT_AGENTS: readonly ChatAgent[] = [
  { id: 'claude', label: 'Claude', blurb: 'Default · ANTHROPIC_API_KEY or Claude sign-in' },
  { id: 'codex', label: 'Codex', blurb: 'OPENAI_API_KEY or Codex sign-in' }
] as const

export const DEFAULT_CHAT_AGENT: ChatAgentId = 'claude'

/** Resolve a stored/unknown agent id to a supported {@link ChatAgentId},
 *  falling back to the default so a stale setting never breaks the composer. */
export function resolveChatAgent(id: string | null | undefined): ChatAgentId {
  return CHAT_AGENTS.find((a) => a.id === id)?.id ?? DEFAULT_CHAT_AGENT
}

/** Resolve the global preference against the installed-agent probe. An explicit
 * preference is preserved even when unavailable so the server can return its
 * typed, actionable error. Only an untouched setting auto-picks Claude (when
 * present) or the first installed Chat-capable agent. */
export function pickChatAgent(
  preferred: ChatAgentId | null | undefined,
  detectedAgents: readonly TuiAgent[] | null
): ChatAgentId {
  if (preferred) return resolveChatAgent(preferred)
  if (detectedAgents) {
    return CHAT_AGENTS.find((candidate) => detectedAgents.includes(candidate.id))?.id ?? DEFAULT_CHAT_AGENT
  }
  return DEFAULT_CHAT_AGENT
}

/** A model offered in the Chat model picker. `id` is the Anthropic model id sent
 *  to `/api/chat/stream`; `label`/`blurb` are display-only. */
export type ChatModel = { id: string; label: string; blurb: string }

/** The models the Chat picker offers — CLAUDE-only (spec 394: non-Claude agents
 *  run their own server-side default model, so this picker is hidden when
 *  another agent is selected). All current Claude models support extended
 *  thinking, so the thinking toggle applies to any of them. Default = Sonnet,
 *  the server's own default (`ChatAgent::Claude.default_model()`). */
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

/** One delta from `/api/chat/stream` — mirrors the server's compact SSE events.
 *  `context` (spec 009 #361) leads the stream on workspace-backed requests:
 *  `missing` means the server could not gather the repo snapshot and the UI
 *  should warn instead of leaving the model to apologize. */
export type ChatStreamDelta =
  | { type: 'text'; text: string }
  | { type: 'thinking'; text: string }
  | { type: 'context'; state: 'ok' | 'missing' }
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
async function sendChat(
  messages: ChatTurn[],
  opts?: {
    workdir?: string
    repoId?: string
    repoSlug?: string
    mode?: IntakeMode
    stage?: number
    agent?: ChatAgentId
  }
): Promise<string> {
  const url = await apiUrl('/api/chat')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders())
    },
    body: JSON.stringify(buildChatBody(messages, opts))
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
    /** Spec 009 (#361): selected workspace's repo id — the server resolves the
     *  repo's host from it and gathers context over SSH for remote projects. */
    repoId?: string
    repoSlug?: string
    model?: string
    thinking?: boolean
    /** Spec 394: which agent runs the turn. Omitting it ⇒ the server resolves
     *  `chat.toml` → Claude (the pre-setting behavior). */
    agent?: ChatAgentId
    /** Spec 008 F2: which intake this turn runs — `'fast'` (default) or
     *  `'socratic'`. Omitting it is the byte-identical Fast path. */
    mode?: IntakeMode
    /** Spec 008 F2: the socratic pass (1..=5); ignored by the server for Fast. */
    stage?: number
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
    // Send only the wire shape; any UI-only fields (e.g. stored `thinking`) are
    // dropped so the server gets a clean `{role, content}[]`.
    body: JSON.stringify(
      buildChatStreamBody(
        messages.map((m) => ({ role: m.role, content: m.content })),
        opts
      )
    ),
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
    } else if (ev.type === 'context') {
      // Status signal only — nothing to accumulate; the store owns the state.
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

/** How the confirmed draft is filed (spec 003). `single` = one issue with the
 *  sub-tasks as a priority checklist; `per_task` = one issue per task. */
export type IssueSplit = 'single' | 'per_task'

/** One task in an editable draft plan (spec 003). */
export type DraftTask = { title: string; detail: string; priority: 'high' | 'medium' | 'low' }

/** The editable draft the preview endpoint returns BEFORE any issue is filed.
 *  `body` is the composed single-issue markdown — a preview of the default split.
 *  `problem`/`goal` (spec 006 F2) are the SDD framing — passthrough only (no
 *  editor; the composed `body` preview shows them rendered): they must survive
 *  the preview → Confirm round-trip or every previewed plan silently loses the
 *  SDD shape (C4). */
export type DraftPlan = {
  title: string
  summary: string
  problem?: string | null
  goal?: string | null
  tasks: DraftTask[]
  body: string
}

/** Decode the server's typed error envelope — `{error:string}` or
 *  `{error:{message}}` — into a message, falling back to `fallback`. */
function decodeError(text: string, fallback: string): string {
  try {
    const parsed = JSON.parse(text) as { error?: { message?: string } | string }
    if (parsed.error && typeof parsed.error === 'object') return parsed.error.message ?? fallback
    if (typeof parsed.error === 'string') return parsed.error
  } catch {
    if (text) return text
  }
  return fallback
}

/**
 * `POST /api/chat/issues/preview` (spec 003) — extract the agreed feature plan
 * from the transcript and return it as an editable DRAFT. Files NOTHING; the UI
 * shows it, the user edits/regenerates, then {@link createIssuesFromChat} files
 * the (edited) plan. Same typed-error decoding as the other chat calls.
 */
export async function previewIssuesFromChat(
  messages: ChatTurn[],
  opts?: { workdir?: string; repoSlug?: string; agent?: ChatAgentId }
): Promise<DraftPlan> {
  const url = await apiUrl('/api/chat/issues/preview')
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(await authHeaders())
    },
    body: JSON.stringify({
      messages,
      workdir: opts?.workdir,
      repo_slug: normalizeRepoSlug(opts?.repoSlug) || undefined,
      // Spec 394: the SAME agent that ran the conversation extracts the plan.
      agent: opts?.agent
    })
  })
  const text = await res.text()
  if (!res.ok) throw new Error(decodeError(text, `chat issues preview ${res.status}`))
  const parsed = (text ? JSON.parse(text) : {}) as Partial<DraftPlan>
  return {
    title: parsed.title ?? '',
    summary: parsed.summary ?? '',
    // Spec 006 F2 (C4): keep the SDD fields on the stored draft so Confirm can
    // post them back — this mapping is where an omission would drop them.
    problem: parsed.problem ?? null,
    goal: parsed.goal ?? null,
    tasks: (parsed.tasks ?? []).map((t) => ({
      title: t?.title ?? '',
      detail: t?.detail ?? '',
      priority: (t?.priority as DraftTask['priority']) ?? 'medium'
    })),
    body: parsed.body ?? ''
  }
}

/** One created issue. `url` is the issue link; `id` is the tracker's stable
 *  handle (a GitHub issue number is implicit in the url; Linear sends its
 *  human identifier like `ENG-42`). */
type CreatedIssue = { title: string; url: string; id?: string }

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
  opts?: {
    workdir?: string
    repoSlug?: string
    provider?: IssueProvider
    /** Spec 003: a user-edited draft. When present the server files it VERBATIM
     *  (skips re-extraction) — the what-you-see-is-what-you-file guarantee. */
    plan?: DraftPlan
    /** Spec 003: one issue + checklist (`single`, default) or one-per-task. */
    split?: IssueSplit
    /** Spec 003: labels for the created issue(s) (GitHub only for now). */
    labels?: string[]
    /** Spec 394: agent for the transcript-extraction fallback (Confirm usually
     *  sends an edited `plan`, which skips the LLM entirely). */
    agent?: ChatAgentId
  }
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
      provider: opts?.provider,
      // Send the edited plan WITHOUT its `body` (the server re-composes it from
      // the tasks); its presence tells the server to skip re-extraction. The
      // SDD fields (spec 006 F2, C4) ride along — this explicit rebuild is the
      // one place a previewed `problem`/`goal` could silently drop before the
      // server composes the filed body.
      plan: opts?.plan
        ? {
            title: opts.plan.title,
            summary: opts.plan.summary,
            problem: opts.plan.problem ?? undefined,
            goal: opts.plan.goal ?? undefined,
            tasks: opts.plan.tasks
          }
        : undefined,
      split: opts?.split,
      labels: opts?.labels && opts.labels.length > 0 ? opts.labels : undefined,
      agent: opts?.agent
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
  throw new Error(decodeError(text, `chat issues ${res.status}`))
}
