// Markdown for annotations made on the AGENT browser screencast (W3).
//
// Distinct from `browser-annotation-output.ts`: that one needs the native pane's
// rich `BrowserGrabPayload` (DOM selector, computed styles, HTML snippet) which the
// screencast picker can't gather — it works off a hit-tested element clip. So this
// is a deliberately small, self-contained shape + formatter.

export type AgentBrowserAnnotationIntent = 'change' | 'question'

/** One annotation the user marked on the agent's browser view. */
export type AgentBrowserAnnotation = {
  /** Stable id for React keys + marker numbering. */
  id: string
  /** `tag#id.class` hint resolved by `node_at_point`. */
  label: string
  intent: AgentBrowserAnnotationIntent
  comment: string
  /** Element clip in CSS-viewport px (for the on-screen marker). */
  clip: { x: number; y: number; width: number; height: number }
  /** Absolute path to the element screenshot the agent can open (from `capture`). */
  screenshotPath?: string
}

function inlineText(content: string): string {
  return content.replace(/\s+/g, ' ').trim()
}

/**
 * Render the pending annotations as a Markdown brief for an agent. Each entry
 * carries the element label, intent, bounds, the user's comment, and — when one
 * was captured — a `Screenshot:` path the agent opens to SEE the element. Empty in
 * → empty string (callers gate Send on a non-empty result).
 */
export function formatAgentBrowserAnnotationsAsMarkdown(
  annotations: readonly AgentBrowserAnnotation[],
  pageUrl: string
): string {
  if (annotations.length === 0) {
    return ''
  }
  const count = annotations.length
  const lines: string[] = [
    `## Browser feedback: ${pageUrl || 'current page'}`,
    '',
    `${count} element${count === 1 ? '' : 's'} annotated on the live agent browser.` +
      ` Where a screenshot path is given, open it to see exactly what I mean.`,
    ''
  ]
  annotations.forEach((annotation, index) => {
    const { clip } = annotation
    lines.push(`### ${index + 1}. ${annotation.label || 'element'}`)
    lines.push(`**Intent:** ${annotation.intent}`)
    lines.push(
      `**Bounds:** x=${Math.round(clip.x)}, y=${Math.round(clip.y)}, ${Math.round(clip.width)}x${Math.round(clip.height)}`
    )
    if (annotation.screenshotPath) {
      lines.push(`**Screenshot:** ${annotation.screenshotPath}`)
    }
    lines.push(`**Feedback:** ${inlineText(annotation.comment)}`)
    lines.push('')
  })
  return lines.join('\n').trimEnd()
}
