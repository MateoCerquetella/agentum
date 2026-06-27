import type { GitHubAssignableUser, GitHubWorkItem, PRComment } from '@/shared/types'

export type MentionOption = {
  login: string
  name?: string | null
  avatarUrl?: string
  source: string
}

export type MentionQuery = {
  atIndex: number
  query: string
}

export function findMentionQuery(value: string, caret: number): MentionQuery | null {
  const beforeCaret = value.slice(0, caret)
  const match = /(^|[\s([{,])@([A-Za-z0-9-]*)$/.exec(beforeCaret)
  if (!match) {
    return null
  }
  const query = match[2] ?? ''
  return {
    atIndex: beforeCaret.length - query.length - 1,
    query
  }
}

export function buildMentionOptions({
  item,
  comments,
  participants,
  assignableUsers
}: {
  item: GitHubWorkItem
  comments: PRComment[]
  participants: GitHubAssignableUser[]
  assignableUsers: GitHubAssignableUser[]
}): MentionOption[] {
  const byLogin = new Map<string, MentionOption>()
  const add = (
    login: string | null | undefined,
    source: string,
    avatarUrl?: string,
    name?: string | null
  ): void => {
    if (!login || login === 'ghost') {
      return
    }
    const key = login.toLowerCase()
    const existing = byLogin.get(key)
    if (existing) {
      if (!existing.avatarUrl && avatarUrl) {
        existing.avatarUrl = avatarUrl
      }
      if (!existing.name && name) {
        existing.name = name
      }
      return
    }
    byLogin.set(key, { login, source, avatarUrl, name })
  }

  add(item.author, item.type === 'pr' ? 'PR author' : 'Issue author')
  for (const comment of comments) {
    add(comment.author, 'Commenter', comment.authorAvatarUrl)
  }
  for (const user of participants) {
    add(user.login, 'Participant', user.avatarUrl, user.name)
  }
  for (const user of assignableUsers) {
    add(user.login, 'Team member', user.avatarUrl, user.name)
  }

  return Array.from(byLogin.values())
}

export function filterMentionOptions(options: MentionOption[], query: string): MentionOption[] {
  const normalizedQuery = query.toLowerCase()
  const filtered = normalizedQuery
    ? options.filter(
        (option) =>
          option.login.toLowerCase().includes(normalizedQuery) ||
          (option.name ?? '').toLowerCase().includes(normalizedQuery)
      )
    : options
  return filtered.slice(0, 8)
}
