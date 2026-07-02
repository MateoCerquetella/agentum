// Spec 006 F1 (AC 3): deterministic issue-body assembly for the composer's
// "Create GitHub issue" form — NO agent call. "Context" is DEFINED as exactly
// the composer's typed agent-prompt field and note field; when the body
// textarea is left blank, what the composer already has in hand becomes the
// issue description instead of filing a bare issue (#232's symptom).

/**
 * Compose the auto-filled issue body from the composer's context fields.
 *
 * - both blank (after trim) → `undefined` — today's bodyless create, pinned.
 * - otherwise → `'## Context'` + the trimmed prompt (when present) + a
 *   `**Note:** …` line (when present), joined by blank lines, no trailing
 *   newline. Both present renders exactly:
 *   `## Context\n\n<prompt>\n\n**Note:** <note>`.
 */
export function composeIssueContextBody(agentPrompt: string, note: string): string | undefined {
  const prompt = agentPrompt.trim()
  const trimmedNote = note.trim()
  if (!prompt && !trimmedNote) {
    return undefined
  }
  const sections = [
    '## Context',
    ...(prompt ? [prompt] : []),
    ...(trimmedNote ? [`**Note:** ${trimmedNote}`] : [])
  ]
  return sections.join('\n\n')
}

/**
 * Label-picker fallback when `GET /api/github/labels` errors (spec 006 F1,
 * D2): the canonical `type/*` + `priority/*` set from `.github/labels.sh`,
 * which keeps the repo's live set synced to these names.
 */
export const STATIC_FALLBACK_LABELS = [
  'type/feat',
  'type/fix',
  'type/perf',
  'type/refactor',
  'type/docs',
  'type/test',
  'type/chore',
  'priority/p0',
  'priority/p1',
  'priority/p2',
  'priority/p3'
] as const
