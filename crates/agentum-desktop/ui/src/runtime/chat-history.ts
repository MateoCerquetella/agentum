// Local conversation history for the Chat screen. Persisted in `localStorage`
// (per-machine, no backend/migration) — the same client-side persistence pattern
// the planner-tool pick and connection profiles already use. One key holds the
// whole list; chat transcripts are small text, so this stays well within quota.
import type { ChatTurn } from './chat-client'

const STORAGE_KEY = 'agentum.chat.conversations.v1'
/** Hard cap on stored conversations — old ones are pruned (newest kept) so the
 *  single localStorage key can't grow unbounded across months of use. */
const MAX_CONVERSATIONS = 200

/** A transcript turn as stored/rendered — the wire {@link ChatTurn} plus the
 *  optional extended-thinking trace captured alongside an assistant reply. */
export type StoredTurn = ChatTurn & { thinking?: string }

/** One saved conversation. `model`/`thinking` are the settings it was last run
 *  with, so reopening it restores the picker state. */
export type Conversation = {
  id: string
  title: string
  messages: StoredTurn[]
  model: string
  thinking: boolean
  createdAt: number
  updatedAt: number
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
