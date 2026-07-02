import { api } from '@/tauri'
import type { GitHubWorkItemDetails } from '@/shared/types'

export function addIssueCommentForRepo(args: {
  repoId?: string
  repoPath: string
  number: number
  body: string
  type?: 'issue' | 'pr'
}): Promise<Awaited<ReturnType<typeof api.gh.addIssueComment>>> {
  return api.gh.addIssueComment({
    repoPath: args.repoPath,
    repoId: args.repoId,
    number: args.number,
    body: args.body,
    type: args.type
  })
}

export function addPRReviewCommentForRepo(args: {
  repoId?: string
  repoPath: string
  prNumber: number
  commitId: string
  path: string
  line: number
  startLine?: number
  body: string
}): Promise<Awaited<ReturnType<typeof api.gh.addPRReviewComment>>> {
  return api.gh.addPRReviewComment({
    repoPath: args.repoPath,
    repoId: args.repoId,
    prNumber: args.prNumber,
    commitId: args.commitId,
    path: args.path,
    line: args.line,
    startLine: args.startLine,
    body: args.body
  })
}

export function addPRReviewCommentReplyForRepo(args: {
  repoId?: string
  repoPath: string
  prNumber: number
  commentId: number
  body: string
  threadId?: string
  path?: string
  line?: number
}): Promise<Awaited<ReturnType<typeof api.gh.addPRReviewCommentReply>>> {
  return api.gh.addPRReviewCommentReply({
    repoPath: args.repoPath,
    repoId: args.repoId,
    prNumber: args.prNumber,
    commentId: args.commentId,
    body: args.body,
    threadId: args.threadId,
    path: args.path,
    line: args.line
  })
}

export function setPRFileViewedForRepo(args: {
  repoId?: string
  repoPath: string
  prNumber: number
  pullRequestId: string
  path: string
  viewed: boolean
}): Promise<boolean> {
  return api.gh.setPRFileViewed({
    repoPath: args.repoPath,
    repoId: args.repoId,
    prNumber: args.prNumber,
    pullRequestId: args.pullRequestId,
    path: args.path,
    viewed: args.viewed
  })
}

export function getWorkItemDetailsForRepo(args: {
  repoId?: string
  repoPath: string
  number: number
  type: 'issue' | 'pr'
}): Promise<GitHubWorkItemDetails | null> {
  return api.gh.workItemDetails({
    repoPath: args.repoPath,
    repoId: args.repoId,
    number: args.number,
    type: args.type
  })
}
