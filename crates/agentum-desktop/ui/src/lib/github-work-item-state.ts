import type { GitHubWorkItem } from '@/shared/types'

export function getStateLabel(item: GitHubWorkItem): string {
  if (item.type === 'pr') {
    if (item.state === 'merged') {
      return 'Merged'
    }
    if (item.state === 'draft') {
      return 'Draft'
    }
    if (item.state === 'closed') {
      return 'Closed'
    }
    return 'Open'
  }
  return item.state === 'closed' ? 'Closed' : 'Open'
}

export function getStateTone(item: GitHubWorkItem): string {
  if (item.type === 'pr') {
    if (item.state === 'merged') {
      return 'border-purple-500/30 bg-purple-500/10 text-purple-600 dark:text-purple-300'
    }
    if (item.state === 'draft') {
      return 'border-slate-500/30 bg-slate-500/10 text-slate-600 dark:text-slate-300'
    }
    if (item.state === 'closed') {
      return 'border-rose-500/30 bg-rose-500/10 text-rose-600 dark:text-rose-300'
    }
    return 'border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
  }
  if (item.state === 'closed') {
    return 'border-rose-500/30 bg-rose-500/10 text-rose-600 dark:text-rose-300'
  }
  return 'border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
}
