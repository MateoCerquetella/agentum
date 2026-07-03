// Module-singleton store for the Chat screen. Conversations AND in-flight
// streams live here — outside React — so a reply keeps streaming when ChatPage
// unmounts (view switch, hub tab change, window blur). The page re-subscribes on
// mount and picks the live transcript back up mid-stream. Persistence stays in
// `chat-history` (localStorage), debounced across the per-token updates.
//
// Consumed via `useSyncExternalStore(subscribeChat, getChatSnapshot)`; every
// mutation replaces the snapshot object immutably so React sees one stable
// reference per change.
//
// Deliberately a hand-rolled module store rather than a zustand slice: it
// preserves the exact `agentum.chat.conversations.v1` storage contract that
// chat-history owns, and the in-flight streams are imperative async processes
// holding AbortControllers — not serializable app state.
import {
  type IntakeMode,
  type IntakeState,
  normalizeIntake,
  resolveIntakeAfterReply,
  SOCRATIC_FIRST_STAGE,
  stripSocraticControl
} from '../lib/socratic-intake'
import { type ChatStreamDelta, streamChat } from './chat-client'
import {
  type Conversation,
  type FiledResult,
  loadConversations,
  newConversationId,
  saveConversations,
  type StoredTurn,
  titleFromMessages,
  upsertConversation
} from './chat-history'

export type ChatSnapshot = {
  conversations: Conversation[]
  /** Conversation ids with a reply currently streaming in. */
  streaming: Readonly<Record<string, true>>
  /** Last stream failure per conversation — cleared on the next send. */
  errors: Readonly<Record<string, string>>
}

/** Drop a trailing still-empty assistant placeholder — the residue of a reload
 *  mid-stream. It would render as an invisible dead turn and can never resume. */
function pruneInterrupted(list: Conversation[]): Conversation[] {
  return list.map((c) => {
    const last = c.messages[c.messages.length - 1]
    if (last && last.role === 'assistant' && !last.content && !last.thinking && !last.filed) {
      return { ...c, messages: c.messages.slice(0, -1) }
    }
    return c
  })
}

let snapshot: ChatSnapshot = {
  conversations: pruneInterrupted(loadConversations()),
  streaming: {},
  errors: {}
}

const listeners = new Set<() => void>()
const aborts = new Map<string, AbortController>()

const SAVE_DEBOUNCE_MS = 400
/** Under continuous streaming the debounce alone would never settle (every
 *  token resets it — more so with several background streams), so force a
 *  write at least this often while there are unsaved changes. */
const SAVE_MAX_DELAY_MS = 2000
let saveTimer: ReturnType<typeof setTimeout> | null = null
let lastSaveAt = 0

export function subscribeChat(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function getChatSnapshot(): ChatSnapshot {
  return snapshot
}

/** Replace the snapshot and notify. Conversation changes also schedule the
 *  debounced localStorage write (coalescing a stream's per-token updates). */
function commit(next: Partial<ChatSnapshot>): void {
  const persist = next.conversations != null && next.conversations !== snapshot.conversations
  snapshot = { ...snapshot, ...next }
  if (persist) {
    if (saveTimer != null) clearTimeout(saveTimer)
    const overdue = Date.now() - lastSaveAt >= SAVE_MAX_DELAY_MS
    saveTimer = setTimeout(
      () => {
        lastSaveAt = Date.now()
        saveConversations(snapshot.conversations)
      },
      overdue ? 0 : SAVE_DEBOUNCE_MS
    )
  }
  for (const l of listeners) l()
}

function setStreaming(id: string, on: boolean): void {
  const streaming = { ...snapshot.streaming }
  if (on) streaming[id] = true
  else delete streaming[id]
  commit({ streaming })
}

function setStreamError(id: string, message: string | null): void {
  const errors = { ...snapshot.errors }
  if (message) errors[id] = message
  else delete errors[id]
  commit({ errors })
}

function patchConversation(id: string, patch: (c: Conversation) => Conversation): void {
  commit({ conversations: snapshot.conversations.map((c) => (c.id === id ? patch(c) : c)) })
}

/** Stream-write into a conversation's trailing assistant turn. */
function updateLastAssistant(id: string, patch: { content?: string; thinking?: string }): void {
  patchConversation(id, (c) => {
    const msgs = c.messages.slice()
    const last = msgs[msgs.length - 1]
    if (!last || last.role !== 'assistant') return c
    msgs[msgs.length - 1] = { ...last, ...patch }
    return { ...c, messages: msgs }
  })
}

/** Append a completed assistant turn (used for the filed-issues summary). Lives
 *  here so a filing that finishes after the page unmounted still lands. */
export function appendAssistantTurn(id: string, content: string, filed?: FiledResult): void {
  patchConversation(id, (c) => ({
    ...c,
    messages: [...c.messages, { role: 'assistant', content, ...(filed ? { filed } : {}) }],
    updatedAt: Date.now()
  }))
}

export function deleteConversation(id: string): void {
  aborts.get(id)?.abort()
  const errors = { ...snapshot.errors }
  delete errors[id]
  commit({ conversations: snapshot.conversations.filter((c) => c.id !== id), errors })
}

export function stopStream(id: string): void {
  aborts.get(id)?.abort()
}

export function dismissStreamError(id: string): void {
  setStreamError(id, null)
}

/**
 * Send one user turn and stream the reply into the conversation. Creates the
 * conversation when `conversationId` is null. Returns the conversation id
 * synchronously; the stream itself runs detached from any component lifecycle.
 * A second send into a conversation that is already streaming is a no-op.
 */
export function sendChatMessage(opts: {
  conversationId: string | null
  text: string
  model: string
  thinking: boolean
  workdir?: string
  repoId?: string
  /** Spec 008 F2: intake mode for a NEW conversation (`'fast'` default). A
   *  CONTINUING thread inherits its stored mode/stage, so this is ignored for it
   *  — the mode is chosen once, at the entry button (D4: per-feature, no sticky
   *  preference is stored elsewhere). */
  mode?: IntakeMode
}): string {
  const text = opts.text.trim()
  const convoId = opts.conversationId ?? newConversationId()
  if (!text || snapshot.streaming[convoId]) return convoId

  const now = Date.now()
  const userTurn: StoredTurn = { role: 'user', content: text }
  // A conversationId that no longer exists (deleted under the caller's feet)
  // falls through to the create branch WITH the same id, so the reply lands
  // somewhere visible instead of streaming into a no-op patch.
  const existing = snapshot.conversations.find((c) => c.id === convoId)
  // Wire history = prior turns + this user turn (the streamed-into placeholder
  // is excluded). streamChat strips the UI-only fields itself.
  const history = [...(existing?.messages ?? []), userTurn]

  // Spec 008 F2 + #257: a continuing thread inherits its stored intake; a NEW
  // thread starts in the picked mode at pass 1. The stage SENT this turn is the
  // stored (or initial) one. The NEXT stage is no longer advanced eagerly here —
  // it's resolved from the model's trailing control marker when the reply
  // finishes (advance/stay/done), so a vague answer re-runs its pass and the
  // interview converges only when the model says the spec is defined.
  const intakeNow: IntakeState = existing?.intake
    ? normalizeIntake(existing.intake)
    : { mode: opts.mode ?? 'fast', stage: SOCRATIC_FIRST_STAGE }

  const messages: StoredTurn[] = [...history, { role: 'assistant', content: '', thinking: '' }]
  const convo: Conversation = existing
    ? { ...existing, messages, model: opts.model, thinking: opts.thinking, updatedAt: now, intake: intakeNow }
    : {
        id: convoId,
        title: titleFromMessages([userTurn]),
        messages,
        model: opts.model,
        thinking: opts.thinking,
        createdAt: now,
        updatedAt: now,
        repoId: opts.repoId,
        intake: intakeNow
      }

  const ac = new AbortController()
  aborts.set(convoId, ac)
  // One commit for the optimistic turns + streaming flag + cleared error — a
  // send is a single state transition, not three notify passes.
  const errors = { ...snapshot.errors }
  delete errors[convoId]
  commit({
    conversations: upsertConversation(snapshot.conversations, convo),
    streaming: { ...snapshot.streaming, [convoId]: true },
    errors
  })

  let content = ''
  let reasoning = ''
  void streamChat(history, {
    workdir: opts.workdir,
    model: opts.model,
    thinking: opts.thinking,
    // Spec 008 F2: drive the server's per-stage prompt with THIS turn's intake
    // (Fast ignores stage). The stored stage was already advanced for next turn.
    mode: intakeNow.mode,
    stage: intakeNow.stage,
    signal: ac.signal,
    onDelta: (d: ChatStreamDelta) => {
      if (d.type === 'text') content += d.text
      else if (d.type === 'thinking') reasoning += d.text
      else return
      updateLastAssistant(convoId, { content, thinking: reasoning })
    }
  })
    .then(() => {
      // #257: the finished reply's trailing control marker moves the Socratic
      // stage machine (advance / stay / done — resolveIntakeAfterReply falls
      // back to the legacy one-pass advance when no marker is present), and is
      // stripped so the transcript never shows the machine channel. An aborted
      // stream skips this, so the same pass re-runs on the next turn. Also
      // bumps updatedAt so the finished conversation floats to the top.
      const intakeAfter = resolveIntakeAfterReply(intakeNow, content)
      const stripped = stripSocraticControl(content)
      patchConversation(convoId, (c) => {
        let msgs = c.messages
        if (stripped !== content) {
          msgs = msgs.slice()
          const last = msgs[msgs.length - 1]
          if (last && last.role === 'assistant') {
            msgs[msgs.length - 1] = { ...last, content: stripped }
          }
        }
        return { ...c, messages: msgs, intake: intakeAfter, updatedAt: Date.now() }
      })
    })
    .catch((e: unknown) => {
      // Aborted (Stop) keeps whatever streamed; a real failure surfaces the
      // server's reason. Either way, drop a still-empty assistant turn so we
      // never leave a blank bubble in the transcript.
      if (!ac.signal.aborted) {
        setStreamError(convoId, e instanceof Error ? e.message : String(e))
      }
      patchConversation(convoId, (c) => {
        const msgs = c.messages.slice()
        const last = msgs[msgs.length - 1]
        if (last && last.role === 'assistant' && !last.content && !last.thinking) msgs.pop()
        return { ...c, messages: msgs }
      })
    })
    .finally(() => {
      if (aborts.get(convoId) === ac) aborts.delete(convoId)
      setStreaming(convoId, false)
    })

  return convoId
}
