import React from 'react'
import { Badge } from '@/components/ui/badge'
import { LayoutGrid } from 'lucide-react'
import { cn } from '@/lib/utils'
import { gh } from '@/tauri/gh'
import { getProjectBinding } from '@/runtime/github-projects-client'
import { subscribeServerEvents } from '@/runtime/server-events-bus'
import { worktreesReconcileGithubStatus } from '@/runtime/server-worktree-client'
import {
  parseIssueRef,
  resolveIssueProjectStatus,
  statusCacheKey,
  type IssueRef,
  type ProjectBindingRef,
  type StatusCacheEntry
} from '@/lib/issue-project-status'

// Spec 018 (#365): the issue hover card's GitHub Project **Status** chip — the
// board column (Todo / In Progress / …) for the linked issue when its repo has
// a Projects v2 binding. GitHub is the single source of lifecycle truth for a
// linked worktree (#399); local trackerPhase is only a cache/retry guard and is
// never rendered beside this chip.

// App-session caches (module-level → shared across every card, survive
// open/close): binding per repo slug, status per issue. Rapid re-hovers hit
// these with no network (AC 3), but status entries expire after
// STATUS_STALE_AFTER_MS — the column moves while a run is coding, and a
// forever-cache showed stale "Backlog" for an In-Progress issue (#379).
const bindingCache = new Map<string, ProjectBindingRef>()
const statusCache = new Map<string, StatusCacheEntry>()

/** Drop the cached Projects status for one issue so the next render refetches
 *  immediately — callers react to `tracker.phase_changed` bus events, where
 *  waiting out the 30s TTL would visibly lag the board. */
export function invalidateIssueProjectStatus(issueUrl: string | undefined | null): void {
  const ref = parseIssueRef(issueUrl)
  if (ref) {
    statusCache.delete(statusCacheKey(ref.slug, ref.number))
  }
}

async function fetchBinding(
  ref: IssueRef,
  workdir: string,
  repoId: string | undefined
): Promise<ProjectBindingRef> {
  // Pass the slug hint (zero git I/O server-side) + repoId (SSH-repo binding,
  // spec 020). null binding = unbound repo.
  const { binding } = await getProjectBinding({
    workdir,
    slug: ref.slug,
    repoId
  })
  if (!binding) {
    return null
  }
  return { projectId: binding.projectId, statusFieldId: binding.statusFieldId }
}

async function fetchStatus(
  ref: IssueRef,
  binding: NonNullable<ProjectBindingRef>
): Promise<{ status: string | null; statusOptionId: string | null }> {
  const res = (await gh.issueProjectStatus({
    owner: ref.owner,
    repo: ref.repo,
    number: ref.number,
    projectId: binding.projectId,
    statusFieldId: binding.statusFieldId
  })) as {
    ok?: boolean
    status?: unknown
    statusOptionId?: unknown
    error?: { message?: unknown }
  } | null
  if (res?.ok === true) {
    return {
      status: typeof res.status === 'string' ? res.status : null,
      statusOptionId: typeof res.statusOptionId === 'string' ? res.statusOptionId : null
    }
  }
  const message = res?.error?.message
  throw new Error(
    typeof message === 'string' && message.trim() ? message : 'GitHub status read failed'
  )
}

/** Resolve the issue's Project Status while `open`. Returns GitHub's option
 *  name plus any sync warning. Tracker events invalidate and refetch this
 *  issue; `sync_pending` preserves the warning until an acknowledged
 *  `phase_changed` confirms the transition (#399). */
export function useIssueProjectStatus(input: {
  open: boolean
  issueUrl?: string
  workdir?: string
  repoId?: string
  worktreeId?: string
}): { status: string | null; warning: string | null } {
  const { open, issueUrl, workdir, repoId, worktreeId } = input
  // #379 perf (stale-while-revalidate): paint the last-known column
  // IMMEDIATELY from the cache — even a stale entry — and let the resolve
  // below swap in the fresh value. Blocking on the refetch made every
  // hover/open past the TTL feel slow (a full `gh` GraphQL round trip).
  const [result, setResult] = React.useState(() => {
    const ref = parseIssueRef(issueUrl)
    const cached = ref ? statusCache.get(statusCacheKey(ref.slug, ref.number)) : null
    return { status: cached?.status ?? null, warning: cached?.warning ?? null }
  })

  React.useEffect(() => {
    if (!open) {
      return
    }
    const ref = parseIssueRef(issueUrl)
    if (!ref || !workdir) {
      return
    }
    let cancelled = false
    const peeked = statusCache.get(statusCacheKey(ref.slug, ref.number))
    if (peeked) {
      setResult({ status: peeked.status, warning: peeked.warning })
    }
    let firstResolve = true
    const resolve = (forceRefresh = false, pendingWarning: string | null = null) => {
      void resolveIssueProjectStatus(
        ref,
        {
          bindingCache,
          statusCache,
          getBinding: (r) => fetchBinding(r, workdir, repoId),
          getStatus: fetchStatus
        },
        { forceRefresh: forceRefresh || firstResolve }
      ).then((next) => {
        firstResolve = false
        if (!cancelled) {
          setResult({ status: next.status, warning: next.warning ?? pendingWarning })
        }
        if (worktreeId && next.statusOptionId) {
          void worktreesReconcileGithubStatus(worktreeId, next.statusOptionId).catch((error) => {
            if (!cancelled) {
              const detail = error instanceof Error ? error.message : String(error)
              setResult((current) => ({
                ...current,
                warning: `GitHub status is live, but Agentum could not reconcile its local cache: ${detail}. Reload after checking file permissions.`
              }))
            }
          })
        }
      })
    }
    resolve()
    const unsubscribe = subscribeServerEvents({
      onEvent: (ev) => {
        if (
          ev.kind !== 'tracker.phase_changed' &&
          ev.kind !== 'tracker.blocked' &&
          ev.kind !== 'tracker.sync_pending'
        ) {
          return
        }
        const url = (ev.payload as { tracker_url?: unknown } | null | undefined)?.tracker_url
        const evRef = typeof url === 'string' ? parseIssueRef(url) : null
        if (!evRef || evRef.slug !== ref.slug || evRef.number !== ref.number) {
          return
        }
        invalidateIssueProjectStatus(url as string)
        let pendingWarning: string | null = null
        if (ev.kind === 'tracker.sync_pending') {
          const reason = (ev.payload as { reason?: unknown } | null | undefined)?.reason
          pendingWarning = `GitHub status sync pending: ${typeof reason === 'string' ? reason : 'transition was not acknowledged'}. Check gh authentication and the Project binding; Agentum will retry.`
          setResult((current) => ({ ...current, warning: pendingWarning }))
        }
        resolve(true, pendingWarning)
      }
    })
    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [open, issueUrl, workdir, repoId, worktreeId])

  return result
}

export function IssueProjectStatusChip({
  status
}: {
  status: string | null
}): React.JSX.Element | null {
  if (!status) {
    return null
  }
  return (
    <Badge
      variant="outline"
      className={cn(
        'h-4 gap-1 rounded px-1.5 text-[9px] font-medium leading-none [&>svg]:size-2.5',
        'border-indigo-500/25 bg-indigo-500/5 text-indigo-600 dark:text-indigo-300'
      )}
    >
      <LayoutGrid />
      <span>{status}</span>
    </Badge>
  )
}
