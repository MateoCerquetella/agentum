import type {
  GitBranchChangeEntry,
  GitDiffResult,
  GitHubPRFile,
  GitHubPRFileContents,
  PRCheckDetail
} from '@/shared/types'

export function isPRFileViewed(file: GitHubPRFile): boolean {
  return file.viewerViewedState === 'VIEWED'
}

export function mapPRFileStatus(status: GitHubPRFile['status']): GitBranchChangeEntry['status'] {
  switch (status) {
    case 'added':
      return 'added'
    case 'removed':
      return 'deleted'
    case 'renamed':
      return 'renamed'
    case 'copied':
      return 'copied'
    case 'changed':
    case 'modified':
    case 'unchanged':
      return 'modified'
  }
}

export function getPRFileSectionKey(path: string): string {
  return `combined-commit:${path}`
}

export function gitHubPRFileToBranchEntry(file: GitHubPRFile): GitBranchChangeEntry {
  return {
    path: file.path,
    oldPath: file.oldPath,
    status: mapPRFileStatus(file.status),
    added: file.additions,
    removed: file.deletions
  }
}

export function getPRFileDiffResult(contents: GitHubPRFileContents): GitDiffResult {
  if (contents.originalIsBinary) {
    return {
      kind: 'binary',
      originalContent: contents.original,
      modifiedContent: contents.modified,
      originalIsBinary: true,
      modifiedIsBinary: contents.modifiedIsBinary
    }
  }
  if (contents.modifiedIsBinary) {
    return {
      kind: 'binary',
      originalContent: contents.original,
      modifiedContent: contents.modified,
      originalIsBinary: false,
      modifiedIsBinary: true
    }
  }

  return {
    kind: 'text',
    originalContent: contents.original,
    modifiedContent: contents.modified,
    originalIsBinary: false,
    modifiedIsBinary: false
  }
}

export function getCheckDetailsKey(check: PRCheckDetail): string {
  return String(check.checkRunId ?? check.workflowRunId ?? check.url ?? check.name)
}

export function findNearestBraceBlock(
  lines: string[],
  targetLine: number
): { startLine: number; endLine: number } | null {
  const stack: number[] = []
  const ranges: { startLine: number; endLine: number }[] = []
  const targetIndex = targetLine - 1

  lines.forEach((line, lineIndex) => {
    for (const character of line) {
      if (character === '{') {
        stack.push(lineIndex)
      } else if (character === '}') {
        const startLine = stack.pop()
        if (startLine !== undefined && startLine <= lineIndex) {
          ranges.push({ startLine: startLine + 1, endLine: lineIndex + 1 })
        }
      }
    }
  })

  const containingRange = ranges
    .filter((range) => range.startLine - 1 <= targetIndex && targetIndex <= range.endLine - 1)
    .sort((a, b) => a.endLine - a.startLine - (b.endLine - b.startLine))[0]

  if (containingRange) {
    return containingRange
  }

  return (
    ranges
      .filter(
        (range) => range.startLine - 1 >= targetIndex && range.startLine - 1 - targetIndex <= 8
      )
      .sort((a, b) => a.startLine - b.startLine)[0] ?? null
  )
}

export function getPRFileContentCacheKey(args: {
  repoPath: string
  repoId: string
  prNumber: number
  file: GitHubPRFile
  headSha: string
  baseSha: string
}): string {
  return [
    args.repoId,
    args.prNumber,
    args.file.path,
    args.file.oldPath ?? '',
    args.file.status,
    args.headSha,
    args.baseSha
  ].join('\0')
}

export function getWorkItemDetailsCacheKey(args: {
  repoPath: string
  repoId: string
  issueSourcePreference: string | undefined
  type: 'issue' | 'pr'
  number: number
}): string {
  // Why: include all axes that change which (repo, item) the IPC resolves to.
  // `\0` separator avoids ambiguity between fields that may contain `:` or `/`.
  return [args.repoId, args.issueSourcePreference ?? 'auto', args.type, args.number].join('\0')
}
