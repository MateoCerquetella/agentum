import type { SddSourceKind } from '@/runtime/sdd-client'
import { useAppStore } from '@/store'

const NEW_SPEC_REQUESTED_EVENT = 'agentum:new-spec-requested'

export type NewSpecPrefillIntent = {
  requestId: string
  repoId: string
  title: string
  goal: string
  sourceKind: SddSourceKind
  sourceReference: string
}

export type NewSpecWorkItem = {
  repoId: string
  title: string
  provider: 'github' | 'linear' | 'jira' | 'unsupported'
  reference: string
  goal?: string
}

let pendingIntent: NewSpecPrefillIntent | null = null

function workItemGoal(item: NewSpecWorkItem): string {
  const requestedGoal = item.goal?.trim()
  if (requestedGoal) return requestedGoal
  const title = item.title.trim()
  if (item.provider === 'unsupported') {
    return `Author a specification for ${title}.\n\nSource work item: ${item.reference.trim()}`
  }
  return `Author a specification for ${title}.`
}

/**
 * The only tracker-to-work entry point. It switches to the project-owned Specs
 * page and leaves a durable-in-renderer prefill for that page's New Spec dialog.
 * No workspace, worktree, agent, provider configuration, or external mutation
 * occurs here.
 */
export function requestNewSpecFromWorkItem(item: NewSpecWorkItem): NewSpecPrefillIntent {
  const repoId = item.repoId.trim()
  const title = item.title.trim()
  const reference = item.reference.trim()
  if (!repoId || !title || !reference) {
    throw new Error('New Spec work-item intent requires repository, title, and source identity.')
  }
  const sourceKind: SddSourceKind =
    item.provider === 'unsupported' ? 'markdown' : item.provider
  const intent: NewSpecPrefillIntent = {
    requestId: crypto.randomUUID(),
    repoId,
    title,
    goal: workItemGoal(item),
    sourceKind,
    sourceReference: item.provider === 'unsupported' ? '' : reference
  }
  pendingIntent = intent
  useAppStore.getState().openProjectHub(repoId, 'specs')
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event(NEW_SPEC_REQUESTED_EVENT))
  }
  return intent
}

export function consumeNewSpecPrefill(repoId: string): NewSpecPrefillIntent | null {
  if (pendingIntent?.repoId !== repoId) return null
  const intent = pendingIntent
  pendingIntent = null
  return intent
}

/** Subscribe only from the page presentation of SddWorkspaceBar. A hidden
 * terminal bar must never consume an intent meant for the visible Specs page. */
export function subscribeNewSpecPrefill(
  repoId: string,
  onIntent: (intent: NewSpecPrefillIntent) => void
): () => void {
  const deliver = (): void => {
    const intent = consumeNewSpecPrefill(repoId)
    if (intent) onIntent(intent)
  }
  deliver()
  if (typeof window === 'undefined') return () => undefined
  window.addEventListener(NEW_SPEC_REQUESTED_EVENT, deliver)
  return () => window.removeEventListener(NEW_SPEC_REQUESTED_EVENT, deliver)
}
