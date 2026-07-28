import { addIssueCommentForRepo } from '@/lib/github-repo-operations'
import React, { useCallback, useState } from 'react'
import { LoaderCircle, Send } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useMountedRef } from '@/hooks/useMountedRef'
import { cn } from '@/lib/utils'
import { GitHubMarkdownComposer } from '@/components/github/GitHubMarkdownComposer'
import type { PRComment } from '@/shared/types'

export function GHCommentComposer({
  className,
  repoPath,
  repoId,
  issueNumber,
  itemType,
  onCommentAdded
}: {
  className?: string
  repoPath: string
  repoId?: string | null
  issueNumber: number
  itemType: 'issue' | 'pr'
  onCommentAdded: (comment: PRComment) => void
}): React.JSX.Element {
  const [body, setBody] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const mountedRef = useMountedRef()

  const handleSubmit = useCallback(async () => {
    const trimmed = body.trim()
    if (!trimmed) {
      return
    }
    setSubmitting(true)
    try {
      const result = await addIssueCommentForRepo({
        repoPath,
        repoId: repoId ?? undefined,
        number: issueNumber,
        body: trimmed,
        type: itemType
      })
      if (!mountedRef.current) {
        return
      }
      if (result.ok) {
        setBody('')
        // Why: use the comment returned by GitHub so the optimistic row shows
        // the real login/avatar immediately instead of waiting for a reopen.
        onCommentAdded(result.comment)
      } else {
        toast.error(result.error ?? 'Failed to add comment')
      }
    } catch (err) {
      if (mountedRef.current) {
        toast.error(err instanceof Error ? err.message : 'Failed to add comment')
      }
    } finally {
      if (mountedRef.current) {
        setSubmitting(false)
      }
    }
  }, [body, mountedRef, repoPath, repoId, issueNumber, itemType, onCommentAdded])

  return (
    <div className={cn('flex flex-col items-start gap-2', className)}>
      <GitHubMarkdownComposer
        value={body}
        onChange={setBody}
        placeholder="Add a comment…"
        disabled={submitting}
        minHeightClassName="min-h-28"
        className="w-full"
        onSubmitShortcut={() => void handleSubmit()}
      />
      <Button
        onClick={handleSubmit}
        disabled={!body.trim() || submitting}
        className="gap-2"
        aria-label="Send comment"
      >
        {submitting ? (
          <LoaderCircle className="size-3.5 animate-spin" />
        ) : (
          <Send className="size-3.5" />
        )}
        Comment
      </Button>
    </div>
  )
}
