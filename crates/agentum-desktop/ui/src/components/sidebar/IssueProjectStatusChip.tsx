import React from 'react'
import { Badge } from '@/components/ui/badge'
import { KanbanSquare } from 'lucide-react'
import { getCachedIssueProjectStatus } from '@/lib/issue-project-status'
import { getIssueProjectStatus } from '../../runtime/github-projects-client'

// Spec 358b: the issue's GitHub Project Status column (Todo / In Progress /
// …), read from the repo's Projects v2 binding. Distinct from IssueStateBadge
// (open/closed) and TrackerPhaseChip (agentum's own pipeline phase): this is
// GitHub's board column, board icon + indigo tone. Renders NOTHING until the
// status resolves, and nothing at all on absence/error (AC 2) — the hover
// card must never show a tracker hiccup.
//
// Laziness (AC 3) comes from placement: the chip lives inside the Radix
// HoverCardContent, which only mounts while the card is open — so mounting IS
// "card opened", and the session cache in `issue-project-status.ts` makes
// every mount after the first hover fetch-free.

export function IssueProjectStatusChip({
  workdir,
  repoId,
  issueNumber
}: {
  workdir: string
  repoId?: string
  issueNumber: number
}): React.JSX.Element | null {
  const [status, setStatus] = React.useState<string | null>(null)

  React.useEffect(() => {
    let cancelled = false
    void getCachedIssueProjectStatus(
      { workdir, repoId, number: issueNumber },
      getIssueProjectStatus
    ).then((resolved) => {
      if (!cancelled) {
        setStatus(resolved)
      }
    })
    return () => {
      cancelled = true
    }
  }, [workdir, repoId, issueNumber])

  if (!status) {
    return null
  }

  return (
    <Badge
      variant="outline"
      className="h-4 gap-1 rounded px-1.5 text-[9px] font-medium leading-none [&>svg]:size-2.5 border-indigo-500/25 bg-indigo-500/5 text-indigo-600 dark:text-indigo-300"
    >
      <KanbanSquare />
      <span>{status}</span>
    </Badge>
  )
}
