// Typed client for the conversational chat route on the embedded agentum-server
// (`POST /api/chat`). Mirrors `board-client.ts`: same loopback endpoint + bearer
// auth (`getServerEndpoint` → `apiUrl`), wire shapes faithful to the server's
// `/api/chat` contract. The endpoint is a Socratic interviewer — it asks
// clarifying questions, then proposes a GitHub task breakdown. This client is
// conversation-only; it never creates tasks.
import { apiUrl, getServerEndpoint } from './server-endpoint'

/** One conversation turn — matches the server's `{role, content}` message shape. */
export type ChatTurn = { role: 'user' | 'assistant'; content: string }

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
