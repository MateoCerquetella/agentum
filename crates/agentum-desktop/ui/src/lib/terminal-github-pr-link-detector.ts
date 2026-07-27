import type { RepoSlug } from './github-links'
import { parseGitHubIssueOrPRLink } from './github-links'

const GITHUB_PR_URL_RE =
  /\bhttps:\/\/(?:www\.)?github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/pull\/\d+(?:[/?#][^\s"'<>]*)?/gi
const HTTPS_SCHEME_PREFIX = 'https://'
const HTTPS_SCHEME_FRAGMENT_LAST_CHARS = new Set('https:/'.split(''))
const TRAILING_TERMINAL_PUNCTUATION_RE = /[),.;\]}]+$/
const TERMINAL_URL_START_BOUNDARY_RE = /[\s"'([{<=>:;,]/
const MAX_CARRY_LENGTH = 512

export type TerminalGitHubPRLink = {
  url: string
  slug: RepoSlug
  number: number
}

function trimTerminalUrl(candidate: string): string {
  return candidate.replace(TRAILING_TERMINAL_PUNCTUATION_RE, '')
}

function parseTerminalGitHubPRUrl(candidate: string): TerminalGitHubPRLink | null {
  const url = trimTerminalUrl(candidate)
  let parsedUrl: URL
  try {
    parsedUrl = new URL(url)
  } catch {
    return null
  }
  if (
    parsedUrl.protocol !== 'https:' ||
    parsedUrl.port !== '' ||
    parsedUrl.username !== '' ||
    parsedUrl.password !== '' ||
    (parsedUrl.hostname !== 'github.com' && parsedUrl.hostname !== 'www.github.com')
  ) {
    return null
  }
  const parsed = parseGitHubIssueOrPRLink(url)
  if (!parsed || parsed.type !== 'pr') {
    return null
  }
  return { url, slug: parsed.slug, number: parsed.number }
}

function endsWithHttpsSchemePrefixFragment(value: string): string {
  for (let length = Math.min(HTTPS_SCHEME_PREFIX.length - 1, value.length); length > 0; length--) {
    const startIndex = value.length - length
    if (
      value.endsWith(HTTPS_SCHEME_PREFIX.slice(0, length)) &&
      hasTerminalUrlStartBoundary(value, startIndex)
    ) {
      return value.slice(value.length - length)
    }
  }
  return ''
}

function getPotentialGitHubPRCarry(value: string): string {
  let schemeIndex = value.lastIndexOf(HTTPS_SCHEME_PREFIX)
  while (schemeIndex !== -1) {
    if (hasTerminalUrlStartBoundary(value, schemeIndex)) {
      const tail = value.slice(schemeIndex)
      // Never trim the beginning of an overlong URL candidate: doing so could
      // discard the origin and make an embedded GitHub URL look standalone in
      // the next PTY chunk.
      return /\s/.test(tail) || tail.length > MAX_CARRY_LENGTH ? '' : tail
    }
    schemeIndex = value.lastIndexOf(HTTPS_SCHEME_PREFIX, schemeIndex - 1)
  }

  const lastChar = value.at(-1)
  if (!lastChar || !HTTPS_SCHEME_FRAGMENT_LAST_CHARS.has(lastChar)) {
    return ''
  }

  return endsWithHttpsSchemePrefixFragment(value)
}

function hasTerminalUrlStartBoundary(value: string, index: number): boolean {
  return index === 0 || TERMINAL_URL_START_BOUNDARY_RE.test(value.charAt(index - 1))
}

export function createTerminalGitHubPRLinkDetector(): (data: string) => TerminalGitHubPRLink[] {
  let carry = ''
  const seenUrls = new Set<string>()

  return (data: string): TerminalGitHubPRLink[] => {
    const combined = carry ? carry + data : data

    const links: TerminalGitHubPRLink[] = []
    for (const match of combined.matchAll(GITHUB_PR_URL_RE)) {
      const rawUrl = match[0]
      const matchIndex = match.index ?? 0
      const matchEnd = matchIndex + rawUrl.length
      if (!hasTerminalUrlStartBoundary(combined, matchIndex)) {
        continue
      }
      // Why: PTY chunks can split the PR number; wait for a boundary before
      // treating a URL at chunk-end as complete.
      if (matchEnd === combined.length) {
        continue
      }

      const parsed = parseTerminalGitHubPRUrl(rawUrl)
      if (!parsed || seenUrls.has(parsed.url)) {
        continue
      }
      seenUrls.add(parsed.url)
      links.push(parsed)
    }

    carry = getPotentialGitHubPRCarry(combined)
    return links
  }
}
