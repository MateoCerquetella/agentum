import { describe, expect, it } from 'vitest'
import {
  formatAgentBrowserAnnotationsAsMarkdown,
  type AgentBrowserAnnotation
} from './agent-browser-annotation-output'

const base: AgentBrowserAnnotation = {
  id: 'a1',
  label: 'button#submit.primary',
  intent: 'change',
  comment: 'make this   green',
  clip: { x: 10.4, y: 20.6, width: 80.2, height: 30.9 }
}

describe('formatAgentBrowserAnnotationsAsMarkdown', () => {
  it('returns empty string for no annotations', () => {
    expect(formatAgentBrowserAnnotationsAsMarkdown([], 'https://x.test')).toBe('')
  })

  it('includes label, intent, rounded bounds and collapsed feedback', () => {
    const md = formatAgentBrowserAnnotationsAsMarkdown([base], 'https://x.test/page')
    expect(md).toContain('## Browser feedback: https://x.test/page')
    expect(md).toContain('### 1. button#submit.primary')
    expect(md).toContain('**Intent:** change')
    expect(md).toContain('**Bounds:** x=10, y=21, 80x31')
    expect(md).toContain('**Feedback:** make this green')
  })

  it('adds a Screenshot path line only when one was captured', () => {
    const withShot = formatAgentBrowserAnnotationsAsMarkdown(
      [{ ...base, screenshotPath: '/tmp/agentum/shot-1.png' }],
      'https://x.test'
    )
    expect(withShot).toContain('**Screenshot:** /tmp/agentum/shot-1.png')

    const withoutShot = formatAgentBrowserAnnotationsAsMarkdown([base], 'https://x.test')
    expect(withoutShot).not.toContain('**Screenshot:**')
  })

  it('numbers multiple annotations in order', () => {
    const md = formatAgentBrowserAnnotationsAsMarkdown(
      [base, { ...base, id: 'a2', label: 'a.link', intent: 'question' }],
      'https://x.test'
    )
    expect(md).toContain('### 1. button#submit.primary')
    expect(md).toContain('### 2. a.link')
    expect(md).toContain('**Intent:** question')
  })
})
