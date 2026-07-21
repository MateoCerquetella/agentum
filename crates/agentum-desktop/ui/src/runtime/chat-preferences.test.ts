import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  CHAT_MODEL_PREFERENCE_KEY,
  readChatModelPreference,
  writeChatModelPreference
} from './chat-preferences'
import { DEFAULT_CHAT_MODEL } from './chat-client'

describe('chat model preference', () => {
  let values: Map<string, string>

  beforeEach(() => {
    values = new Map()
    vi.stubGlobal('localStorage', {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
      clear: () => values.clear()
    })
  })

  afterEach(() => vi.unstubAllGlobals())

  it('uses the existing Chat key and default', () => {
    expect(CHAT_MODEL_PREFERENCE_KEY).toBe('agentum.chat.model')
    expect(readChatModelPreference()).toBe(DEFAULT_CHAT_MODEL)
  })

  it('round-trips the shared model choice', () => {
    writeChatModelPreference('claude-opus-4-8')
    expect(readChatModelPreference()).toBe('claude-opus-4-8')
  })

  it('keeps working when storage is unavailable', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      }
    })
    expect(readChatModelPreference()).toBe(DEFAULT_CHAT_MODEL)
    expect(() => writeChatModelPreference('claude-opus-4-8')).not.toThrow()
  })
})
