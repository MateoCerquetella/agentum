import type { PRComment } from './types'

export function unresolvedThreadCount(comments?: PRComment[]): number | null {
  if (comments === undefined) {
    return null
  }
  const unresolved = new Set<string>()
  for (const comment of comments) {
    if (!comment.threadId || comment.isResolved !== false) {
      continue
    }
    unresolved.add(comment.threadId)
  }
  return unresolved.size
}
