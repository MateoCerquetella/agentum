import { MentionTextarea } from './pull-request-mention-textarea'
import { type MentionOption } from './pull-request-mentions'
import { addIssueCommentForRepo } from '@/lib/github-repo-operations'
import React, { useCallback, useRef, useState } from 'react'
import { LoaderCircle, Send } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useMountedRef } from '@/hooks/useMountedRef'
import { cn } from '@/lib/utils'
import { isScreenSubmitShortcut } from '@/lib/screen-submit-shortcut'
import type { PRComment } from '../../../shared/types'

export function GHCommentComposer({
  className,
  repoPath,
  repoId,
  issueNumber,
  itemType,
  mentionOptions,
  onCommentAdded
}: {
  className?: string
  repoPath: string
  repoId?: string | null
  issueNumber: number
  itemType: 'issue' | 'pr'
  mentionOptions: MentionOption[]
  onCommentAdded: (comment: PRComment) => void
}): React.JSX.Element {
  const [body, setBody] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const mountedRef = useMountedRef()

  const autoGrow = useCallback(() => {
    const el = textareaRef.current
    if (!el) {
      return
    }
    el.style.height = 'auto'
    el.style.height = `${Math.max(80, Math.min(el.scrollHeight, 240))}px`
  }, [])

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
        requestAnimationFrame(autoGrow)
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
  }, [autoGrow, body, mountedRef, repoPath, repoId, issueNumber, itemType, onCommentAdded])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (isScreenSubmitShortcut(e)) {
        e.preventDefault()
        handleSubmit()
      }
    },
    [handleSubmit]
  )

  return (
    <div className={cn('flex flex-col items-start gap-2', className)}>
      <MentionTextarea
        textareaRef={textareaRef}
        value={body}
        onValueChange={(nextValue) => {
          setBody(nextValue)
          requestAnimationFrame(autoGrow)
        }}
        onKeyDown={handleKeyDown}
        placeholder="Add a comment…"
        rows={4}
        mentionOptions={mentionOptions}
        wrapperClassName="flex min-h-20 w-full items-stretch"
        className="scrollbar-sleek block h-20 max-h-[240px] min-h-20 w-full resize-none overflow-y-auto rounded-md border border-input bg-card px-3 py-2 text-[13px] leading-5 placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
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
