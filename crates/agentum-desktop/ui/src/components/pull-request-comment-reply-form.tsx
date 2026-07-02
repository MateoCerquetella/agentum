import { MentionTextarea } from './pull-request-mention-textarea'
import { type MentionOption } from './pull-request-mentions'
import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { useMountedRef } from '@/hooks/useMountedRef'
import { cn } from '@/lib/utils'
import { isScreenSubmitShortcut } from '@/lib/screen-submit-shortcut'

export function CommentReplyForm({
  className,
  placeholder,
  mentionOptions,
  onCancel,
  onSubmit
}: {
  className?: string
  placeholder: string
  mentionOptions: MentionOption[]
  onCancel: () => void
  onSubmit: (body: string) => Promise<boolean>
}): React.JSX.Element {
  const [body, setBody] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const mountedRef = useMountedRef()

  useEffect(() => {
    textareaRef.current?.focus()
  }, [])

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
      <MentionTextarea
        textareaRef={textareaRef}
        value={body}
        onValueChange={setBody}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            onCancel()
            return
          }
          if (isScreenSubmitShortcut(e)) {
            e.preventDefault()
            void submit()
          }
        }}
        placeholder={placeholder}
        rows={3}
        mentionOptions={mentionOptions}
        className="scrollbar-sleek min-h-20 w-full resize-y rounded-md border border-input bg-transparent px-3 py-2 text-[13px] placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
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
