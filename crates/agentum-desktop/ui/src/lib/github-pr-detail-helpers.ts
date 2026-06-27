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
