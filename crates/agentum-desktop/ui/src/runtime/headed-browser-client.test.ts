import { describe, expect, it } from 'vitest'
import { formatHeadedAnnotationForAgent } from './headed-browser-client'

describe('formatHeadedAnnotationForAgent', () => {
  it('renders a change request with selector, url, and comment', () => {
    const out = formatHeadedAnnotationForAgent({
      comment: 'make this button blue',
      intent: 'change',
      payload: { page: { url: 'https://x.test/app' }, target: { selector: 'button.cta' } }
    })
    expect(out).toContain('Please change')
    expect(out).toContain('button.cta')
    expect(out).toContain('https://x.test/app')
    expect(out).toContain('make this button blue')
  })

  it('uses a question framing for the question intent', () => {
    const out = formatHeadedAnnotationForAgent({
      comment: 'what is this?',
      intent: 'question',
      payload: { target: { selector: '#hero' } }
    })
    expect(out).toContain('Question about')
    expect(out).toContain('#hero')
  })

  it('falls back to tagName then a generic label when no selector', () => {
    expect(formatHeadedAnnotationForAgent({ comment: 'x', payload: { target: { tagName: 'div' } } })).toContain(
      'div'
    )
    expect(formatHeadedAnnotationForAgent({ comment: 'x' })).toContain('the selected element')
  })
})
