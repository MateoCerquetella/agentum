import { describe, expect, it } from 'vitest'
import { createIssuePayload, extractServerErrorMessage } from './github-issue-client'

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

// Spec 020 F3: the create-issue body builder. Absent optionals produce absent
// keys — the pre-020 wire shape must stay byte-identical when nothing new is
// threaded (labels pin predates this: omitted-when-empty since spec 006).

describe('createIssuePayload', () => {
  it('carries only title + workdir when nothing optional is supplied', () => {
    expect(createIssuePayload({ title: 'Add dark mode', workdir: '/home/me/proj' })).toEqual({
      title: 'Add dark mode',
      workdir: '/home/me/proj'
    })
  })

  it('includes body / slug / labels / repoId only when supplied', () => {
    expect(
      createIssuePayload({
        title: 'Add dark mode',
        body: '## Problem',
        workdir: '/srv/proj',
        slug: 'acme/widgets',
        labels: ['type/feature'],
        repoId: 'repo-1'
      })
    ).toEqual({
      title: 'Add dark mode',
      body: '## Problem',
      workdir: '/srv/proj',
      slug: 'acme/widgets',
      labels: ['type/feature'],
      repoId: 'repo-1'
    })
  })

  it('omits an empty labels array (the pre-006 wire shape stays byte-identical)', () => {
    const payload = createIssuePayload({ title: 't', workdir: '/p', labels: [] })
    expect(payload).toEqual({ title: 't', workdir: '/p' })
  })
})
