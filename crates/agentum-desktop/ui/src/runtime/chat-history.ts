// Local conversation history for the Chat screen. Persisted in `localStorage`
// (per-machine, no backend/migration) — the same client-side persistence pattern
// the planner-tool pick and connection profiles already use. One key holds the
// whole list; chat transcripts are small text, so this stays well within quota.
import type { IntakeState } from '../lib/socratic-intake'
import type { ChatAgentId, ChatTurn } from './chat-client'

const STORAGE_KEY = 'agentum.chat.conversations.v1'
/** Hard cap on stored conversations — old ones are pruned (newest kept) so the
 *  single localStorage key can't grow unbounded across months of use. */
const MAX_CONVERSATIONS = 200

/** The outcome of a confirmed draft: what got filed, where. Stored on the
 *  summary turn so the transcript can render a clickable issues card instead of
 *  plain markdown. Structurally mirrors the create response but is declared
 *  here so the storage schema can't drift under a wire-type change. */
export type FiledResult = {
  provider: 'github' | 'linear'
  repo: string | null
  issues: { title: string; url: string; id?: string }[]
  failed: { title: string; error: string }[]
}

/** A transcript turn as stored/rendered — the wire {@link ChatTurn} plus the
 *  optional extended-thinking trace captured alongside an assistant reply and,
 *  on a filing-summary turn, the {@link FiledResult}. `content` always keeps
 *  the markdown fallback so pre-existing chats and old builds render the same
 *  information. */
export type StoredTurn = ChatTurn & { thinking?: string; filed?: FiledResult }

/** One saved conversation. `model`/`thinking` are the settings it was last run
 *  with, so reopening it restores the picker state. `repoId` scopes the thread
 *  to the project it was grounded in (the Project Hub filters by it); optional
 *  because conversations predating the hub carry no scope. */
export type Conversation = {
  id: string
  title: string
  messages: StoredTurn[]
  model: string
  thinking: boolean
  createdAt: number
  updatedAt: number
  repoId?: string
  /** Spec 394: which agent the thread last ran on — DISPLAY metadata only (the
   *  history row shows "Codex" instead of a misleading Claude model label).
   *  The agent itself is the global Settings pick, never a per-conversation
   *  override (spec non-goal), so reopening does NOT restore this. Absent on
   *  pre-394 threads ⇒ Claude. */
  agent?: ChatAgentId
  /** Spec 008 F2: the Fast/Complex intake this thread runs, with the socratic
   *  pass the NEXT turn will use. Persisted here (D1: no new store table) so a
   *  reload resumes a Complex interview at the right pass; absent on pre-008
   *  threads ⇒ Fast (see `normalizeIntake`). */
  intake?: IntakeState
}

/** A unique-enough id without a uuid dependency (time + random suffix). */
export function newConversationId(): string {
  const rand = Math.random().toString(36).slice(2, 8)
  return `c_${Date.now().toString(36)}_${rand}`
}

/** Derive a short, single-line title from the first user turn. */
export function titleFromMessages(messages: StoredTurn[]): string {
  const first = messages.find((m) => m.role === 'user')?.content?.trim()
  if (!first) return 'New chat'
  const oneLine = first.replace(/\s+/g, ' ')
  return oneLine.length > 60 ? `${oneLine.slice(0, 57)}…` : oneLine
}

/** Load all conversations, newest-updated first. Tolerates missing/corrupt
 *  storage (returns `[]`) so a bad write can never break the Chat screen. */
export function loadConversations(): Conversation[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return (parsed as Conversation[])
      .filter(
        (c) =>
          c &&
          typeof c.id === 'string' &&
          Array.isArray(c.messages) &&
          typeof c.updatedAt === 'number'
      )
      .sort((a, b) => b.updatedAt - a.updatedAt)
  } catch {
    return []
  }
}

/** Persist the full list (capped, newest-first). Best-effort: a full/unavailable
 *  store just means history won't survive this session — never a thrown error. */
export function saveConversations(list: Conversation[]): void {
  try {
    const capped = [...list].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, MAX_CONVERSATIONS)
    localStorage.setItem(STORAGE_KEY, JSON.stringify(capped))
  } catch {
    // Storage unavailable (private mode / quota) — degrade to in-memory only.
  }
}

/** Upsert one conversation into a list and return the new list (newest-first).
 *  Pure — the caller owns the state + the `saveConversations` write. */
export function upsertConversation(list: Conversation[], convo: Conversation): Conversation[] {
  const without = list.filter((c) => c.id !== convo.id)
  return [convo, ...without].sort((a, b) => b.updatedAt - a.updatedAt)
}
