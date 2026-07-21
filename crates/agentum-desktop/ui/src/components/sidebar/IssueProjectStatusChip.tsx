import React from 'react'
import { Badge } from '@/components/ui/badge'
import { LayoutGrid } from 'lucide-react'
import { cn } from '@/lib/utils'
import { gh } from '@/tauri/gh'
import { getProjectBinding } from '@/runtime/github-projects-client'
import { subscribeServerEvents } from '@/runtime/server-events-bus'
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
// a Projects v2 binding. Distinct from the open/closed IssueStateBadge and from
// the internal TrackerPhaseChip (agentum's own pipeline phase). Fetched lazily
// on card open + cached per issue for the app session; renders NOTHING when
// unbound, off-project, or on any fetch error (silent absence, AC 2).

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
  const { binding } = await getProjectBinding({ workdir, slug: ref.slug, repoId })
  if (!binding) {
    return null
  }
  return { projectId: binding.projectId, statusFieldId: binding.statusFieldId }
}

async function fetchStatus(
  ref: IssueRef,
  binding: NonNullable<ProjectBindingRef>
): Promise<string | null> {
  // `gh_issue_project_status` → { ok:true, status:<name|null> } | { ok:false, error }.
  // Both a null status and an error envelope mean "no chip".
  const res = (await gh.issueProjectStatus({
    owner: ref.owner,
    repo: ref.repo,
    number: ref.number,
    projectId: binding.projectId,
    statusFieldId: binding.statusFieldId
  })) as { ok?: boolean; status?: unknown } | null
  if (res && res.ok === true && typeof res.status === 'string') {
    return res.status
  }
  return null
}

/** Lazily resolve the issue's Project Status while `open`. Returns the option
 *  name or null (unbound / off-project / error / not open). Live: while open,
 *  a `tracker.phase_changed`/`tracker.blocked` bus event for THIS issue
 *  invalidates the cache and refetches, so engine/MCP-driven transitions
 *  appear without a hover cycle or app restart (#379). */
export function useIssueProjectStatus(input: {
  open: boolean
  issueUrl?: string
  workdir?: string
  repoId?: string
}): string | null {
  const { open, issueUrl, workdir, repoId } = input
  // #379 perf (stale-while-revalidate): paint the last-known column
  // IMMEDIATELY from the cache — even a stale entry — and let the resolve
  // below swap in the fresh value. Blocking on the refetch made every
  // hover/open past the TTL feel slow (a full `gh` GraphQL round trip).
  const [status, setStatus] = React.useState<string | null>(() => {
    const ref = parseIssueRef(issueUrl)
    return ref ? (statusCache.get(statusCacheKey(ref.slug, ref.number))?.status ?? null) : null
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
      setStatus(peeked.status)
    }
    const resolve = () => {
      void resolveIssueProjectStatus(ref, {
        bindingCache,
        statusCache,
        getBinding: (r) => fetchBinding(r, workdir, repoId),
        getStatus: fetchStatus
      }).then((result) => {
        if (!cancelled) {
          setStatus(result)
        }
      })
    }
    resolve()
    const unsubscribe = subscribeServerEvents({
      onEvent: (ev) => {
        if (ev.kind !== 'tracker.phase_changed' && ev.kind !== 'tracker.blocked') {
          return
        }
        const url = (ev.payload as { tracker_url?: unknown } | null | undefined)?.tracker_url
        const evRef = typeof url === 'string' ? parseIssueRef(url) : null
        if (!evRef || evRef.slug !== ref.slug || evRef.number !== ref.number) {
          return
        }
        invalidateIssueProjectStatus(url as string)
        resolve()
      }
    })
    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [open, issueUrl, workdir, repoId])

  return status
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
