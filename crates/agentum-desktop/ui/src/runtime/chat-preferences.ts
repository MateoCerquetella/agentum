import { DEFAULT_CHAT_MODEL } from './chat-client'

/** The existing Chat model preference key. Keep this as the single owner so
 * Chat and contextual drafting surfaces never drift onto separate defaults. */
export const CHAT_MODEL_PREFERENCE_KEY = 'agentum.chat.model'

export function readChatModelPreference(): string {
  try {
    return localStorage.getItem(CHAT_MODEL_PREFERENCE_KEY) || DEFAULT_CHAT_MODEL
  } catch {
    return DEFAULT_CHAT_MODEL
  }
}

export function writeChatModelPreference(model: string): void {
  try {
    localStorage.setItem(CHAT_MODEL_PREFERENCE_KEY, model)
  } catch {
    // Storage can be unavailable; the in-memory picker remains usable.
  }
}
