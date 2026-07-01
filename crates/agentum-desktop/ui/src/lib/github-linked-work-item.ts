import type { GitHubWorkItem } from '../../../shared/types'
import type { LinkedWorkItemSummary } from '@/lib/new-workspace'

// Why: a large issue body must not blow the prompt budget. The composer applies
// its own hard cap downstream (`buildContainedLinkedContextBlock`), but trimming
// here keeps the snapshot self-describing and bounded at the source — mirrors
// the Linear snapshot's `descriptionChars` cap.
export const GITHUB_ISSUE_BODY_MAX_CHARS = 8000

const TRUNCATED_MARKER = '[truncated]'

function normalizeInline(value: string): string {
  return value.replace(/\s+/g, ' ').trim()
}

function truncate(value: string, maxChars: number): string {
  const trimmed = value.trim()
  if (trimmed.length <= maxChars) {
    return trimmed
  }
  const suffix = `\n${TRUNCATED_MARKER}`
  const bodyLimit = Math.max(0, maxChars - suffix.length)
  return `${trimmed.slice(0, bodyLimit).trimEnd()}${suffix}`
}

/**
 * Render a GitHub issue (identity + fetched body) into the bounded plain-text
 * snapshot the composer folds into the agent prompt — the GitHub analogue of
 * `buildLinearIssueContextSnapshot`.
 */
export function buildGithubIssueContextSnapshot(args: {
  number: number
  title: string
  url: string
  body: string
}): string {
  const lines: string[] = [
    'GitHub issue context snapshot',
    `Number: #${args.number}`,
    `Title: ${normalizeInline(args.title)}`,
    `URL: ${normalizeInline(args.url)}`
  ]
  const body = args.body.trim()
  if (body) {
    lines.push('', 'Body:', truncate(body, GITHUB_ISSUE_BODY_MAX_CHARS))
  }
  return lines.join('\n')
}

/**
 * Build the composer's linked-work-item for a GitHub issue, snapshotting the
 * fetched body into `linkedContext` so `buildAgentPromptWithContext` seeds the
 * spawned agent with the spec (not just the URL). Mirrors
 * `buildLinearIssueLinkedWorkItem`. When the body is empty the `linkedContext`
 * is omitted, so the caller's existing title+URL prompt path applies unchanged.
 */
export function buildGithubIssueLinkedWorkItem(
  item: GitHubWorkItem,
  fetched: { title?: string; body: string }
): LinkedWorkItemSummary {
  const trimmedBody = fetched.body.trim()
  const summary: LinkedWorkItemSummary = {
    type: item.type,
    number: item.number,
    title: item.title,
    url: item.url
  }
  if (!trimmedBody) {
    return summary
  }
  return {
    ...summary,
    linkedContext: {
      provider: 'github',
      version: 1,
      renderedText: buildGithubIssueContextSnapshot({
        number: item.number,
        title: fetched.title?.trim() || item.title,
        url: item.url,
        body: fetched.body
      })
    }
  }
}
