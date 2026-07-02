import { describe, expect, it } from 'vitest'
import { extractServerErrorMessage } from './github-issue-client'

// Spec 007: the "Generate description" form surfaces server errors inline —
// most importantly the no-credentials message from /api/github/issues/draft-body
// (`{"error":"No LLM credentials for chat: …"}`), which must reach the user
// verbatim, not as an opaque status code.

describe('extractServerErrorMessage', () => {
  it('unwraps the string error envelope (ApiError::BadRequest)', () => {
    expect(
      extractServerErrorMessage('{"error":"No LLM credentials for chat: set ANTHROPIC_API_KEY"}', 'x')
    ).toBe('No LLM credentials for chat: set ANTHROPIC_API_KEY')
  })

  it('unwraps the object error envelope (ApiError::Custom)', () => {
    expect(
      extractServerErrorMessage('{"error":{"code":"llm_failed","message":"chat model returned 401"}}', 'x')
    ).toBe('chat model returned 401')
  })

  it('falls back to the raw body, then the caller fallback', () => {
    expect(extractServerErrorMessage('plain text failure', 'x')).toBe('plain text failure')
    expect(extractServerErrorMessage('', 'draft description failed (500)')).toBe(
      'draft description failed (500)'
    )
    // JSON without an error field → raw body is still more useful than nothing.
    expect(extractServerErrorMessage('{"other":1}', 'fb')).toBe('{"other":1}')
  })
})
