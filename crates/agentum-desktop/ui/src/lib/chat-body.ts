// Pure wire-body builders for /api/chat and /api/chat/stream — extracted from
// chat-client's fetch calls (spec 009 #361) so "a workspace-selected send
// carries workdir + repo_id" is a plain model test instead of a fetch mock.
// Keep these free of runtime imports: they must stay loadable in bare vitest.
import type { IntakeMode } from './socratic-intake'

export type ChatBodyTurn = { role: 'user' | 'assistant'; content: string }

export type ChatBodyOpts = {
  workdir?: string
  /** Spec 009 (#361): the selected workspace's repo id — lets the server
   *  resolve the repo's HOST and gather context over SSH for remote projects.
   *  Absent (old callers / no workspace) keeps the wire byte-identical. */
  repoId?: string
  repoSlug?: string
  mode?: IntakeMode
  stage?: number
}

/** Body for `POST /api/chat` — exactly the pre-009 shape plus `repo_id`.
 *  `undefined` fields are dropped by JSON.stringify, same as before. */
export function buildChatBody(messages: ChatBodyTurn[], opts?: ChatBodyOpts): Record<string, unknown> {
  return {
    messages,
    workdir: opts?.workdir,
    repo_id: opts?.repoId,
    repo_slug: opts?.repoSlug,
    // Spec 008 F2: intake mode + socratic pass (both serde-default server-side,
    // so omitting them is the byte-identical Fast path).
    mode: opts?.mode,
    stage: opts?.stage
  }
}

/** Body for `POST /api/chat/stream` — the chat body plus the stream-only
 *  fields (model, thinking). Messages are already stripped to the wire shape
 *  by the caller. */
export function buildChatStreamBody(
  messages: ChatBodyTurn[],
  opts?: ChatBodyOpts & { model?: string; thinking?: boolean }
): Record<string, unknown> {
  return {
    messages,
    workdir: opts?.workdir,
    repo_id: opts?.repoId,
    repo_slug: opts?.repoSlug,
    model: opts?.model,
    thinking: opts?.thinking ?? false,
    mode: opts?.mode,
    stage: opts?.stage
  }
}
