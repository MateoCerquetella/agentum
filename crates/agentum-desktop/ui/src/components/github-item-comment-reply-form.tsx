import React, { useCallback, useState } from 'react'
import { Button } from '@/components/ui/button'
import { useMountedRef } from '@/hooks/useMountedRef'
import { cn } from '@/lib/utils'
import { GitHubMarkdownComposer } from '@/components/github/GitHubMarkdownComposer'

export function CommentReplyForm({
  className,
  placeholder,
  onCancel,
  onSubmit
}: {
  className?: string
  placeholder: string
  onCancel: () => void
  onSubmit: (body: string) => Promise<boolean>
}): React.JSX.Element {
  const [body, setBody] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const mountedRef = useMountedRef()

  const submit = useCallback(async () => {
    const trimmed = body.trim()
    if (!trimmed || submitting) {
      return
    }
    setSubmitting(true)
    try {
      const ok = await onSubmit(trimmed)
      if (!mountedRef.current) {
        return
      }
      if (ok) {
        setBody('')
      }
    } finally {
      if (mountedRef.current) {
        setSubmitting(false)
      }
    }
  }, [body, mountedRef, onSubmit, submitting])

  return (
    <div className={cn('rounded-md border border-border/50 bg-background/60 p-2', className)}>
      <GitHubMarkdownComposer
        value={body}
        onChange={setBody}
        placeholder={placeholder}
        disabled={submitting}
        autoFocus
        minHeightClassName="min-h-24"
        onSubmitShortcut={() => void submit()}
      />
      <div className="mt-2 flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" disabled={!body.trim() || submitting} onClick={() => void submit()}>
          {submitting ? 'Posting…' : 'Reply'}
        </Button>
      </div>
    </div>
  )
}
