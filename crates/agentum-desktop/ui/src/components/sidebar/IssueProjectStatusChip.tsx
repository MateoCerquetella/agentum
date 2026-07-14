import React from 'react'
import { Badge } from '@/components/ui/badge'
import { LayoutGrid } from 'lucide-react'
import { cn } from '@/lib/utils'
import { gh } from '@/tauri/gh'
import { getProjectBinding } from '@/runtime/github-projects-client'
import {
  parseIssueRef,
  resolveIssueProjectStatus,
  type IssueRef,
  type ProjectBindingRef
} from '@/lib/issue-project-status'

// Spec 018 (#365): the issue hover card's GitHub Project **Status** chip — the
// board column (Todo / In Progress / …) for the linked issue when its repo has
// a Projects v2 binding. Distinct from the open/closed IssueStateBadge and from
// the internal TrackerPhaseChip (agentum's own pipeline phase). Fetched lazily
// on card open + cached per issue for the app session; renders NOTHING when
// unbound, off-project, or on any fetch error (silent absence, AC 2).

// App-session caches (module-level → shared across every card, survive
// open/close): binding per repo slug, status per issue. A second hover hits
// these and issues no network (AC 3).
const bindingCache = new Map<string, ProjectBindingRef>()
const statusCache = new Map<string, string | null>()

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

/** Lazily resolve the issue's Project Status when the hover card is open.
 *  Returns the option name or null (unbound / off-project / error / not open). */
export function useIssueProjectStatus(input: {
  open: boolean
  issueUrl?: string
  workdir?: string
  repoId?: string
}): string | null {
  const { open, issueUrl, workdir, repoId } = input
  const [status, setStatus] = React.useState<string | null>(null)

  React.useEffect(() => {
    if (!open) {
      return
    }
    const ref = parseIssueRef(issueUrl)
    if (!ref || !workdir) {
      return
    }
    let cancelled = false
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
    return () => {
      cancelled = true
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
