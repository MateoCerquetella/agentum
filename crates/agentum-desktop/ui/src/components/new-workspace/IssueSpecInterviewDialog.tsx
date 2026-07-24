import React, { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowRight, Loader2, RotateCcw, Sparkles } from 'lucide-react'

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { useDetectedAgents } from '@/hooks/useDetectedAgents'
import {
  isSocraticComplete,
  resolveIntakeAfterReply,
  socraticIntake,
  stripSocraticControl,
  type IntakeState
} from '@/lib/socratic-intake'
import { useAppStore } from '@/store'
import type { Repo } from '@/shared/types'
import {
  pickChatAgent,
  previewIssueSpec,
  streamChat,
  type ChatTurn,
  type IssueSpecDraft
} from '@/runtime/chat-client'
import { readChatModelPreference } from '@/runtime/chat-preferences'

const STAGES = ['Who', 'Outcome', 'Why now', 'Done criteria', 'Risks & scope'] as const

export function IssueSpecInterviewDialog({
  open,
  onOpenChange,
  repo,
  repoSlug,
  seedIntent,
  resetVersion,
  onApplyDraft
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  repo: Repo
  repoSlug?: string
  seedIntent: string
  resetVersion: number
  onApplyDraft: (draft: IssueSpecDraft) => void
}): React.JSX.Element {
  const preferredAgent = useAppStore((state) => state.settings?.chatAgent)
  const { detectedIds } = useDetectedAgents()
  const agent = pickChatAgent(preferredAgent, detectedIds)
  const model = agent === 'claude' ? readChatModelPreference() : undefined
  const contextKey = `${repo.id}:${repo.path}:${repoSlug ?? ''}`
  const liveContextKey = useRef(contextKey)
  liveContextKey.current = contextKey
  const guardCurrent = useCallback(() => liveContextKey.current === contextKey, [contextKey])

  const [messages, setMessages] = useState<ChatTurn[]>([])
  const [intake, setIntake] = useState<IntakeState>(socraticIntake)
  const [answer, setAnswer] = useState('')
  const [liveReply, setLiveReply] = useState('')
  const [streaming, setStreaming] = useState(false)
  const [extracting, setExtracting] = useState(false)
  const [contextMissing, setContextMissing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const sessionKey = `${contextKey}:${resetVersion}:${seedIntent.trim()}`
  const activeSession = useRef<string | null>(null)
  const messagesRef = useRef<ChatTurn[]>([])
  const intakeRef = useRef<IntakeState>(socraticIntake())
  const pendingReply = useRef(false)
  const abortRef = useRef<AbortController | null>(null)
  const runId = useRef(0)

  useEffect(() => { messagesRef.current = messages }, [messages])
  useEffect(() => { intakeRef.current = intake }, [intake])

  const askNext = useCallback(async (history: ChatTurn[], state: IntakeState): Promise<void> => {
    if (!guardCurrent() || history.length === 0) return
    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller
    const id = ++runId.current
    pendingReply.current = true
    setStreaming(true)
    setLiveReply('')
    setError(null)
    let partial = ''
    try {
      const result = await streamChat(history, {
        workdir: repo.path,
        repoId: repo.id,
        repoSlug,
        agent,
        model,
        mode: 'socratic',
        stage: state.stage,
        target: 'issue_spec',
        signal: controller.signal,
        onDelta: (delta) => {
          if (id !== runId.current) return
          if (delta.type === 'context') setContextMissing(delta.state === 'missing')
          if (delta.type === 'text') {
            partial += delta.text
            setLiveReply(stripSocraticControl(partial))
          }
        }
      })
      if (id !== runId.current || !guardCurrent()) return
      const visible = stripSocraticControl(result.content).trim()
      const nextMessages = visible
        ? [...history, { role: 'assistant' as const, content: visible }]
        : history
      const nextIntake = resolveIntakeAfterReply(state, result.content)
      messagesRef.current = nextMessages
      intakeRef.current = nextIntake
      setMessages(nextMessages)
      setIntake(nextIntake)
      setLiveReply('')
      pendingReply.current = false
    } catch (cause) {
      if (id !== runId.current || controller.signal.aborted) return
      pendingReply.current = false
      setError(cause instanceof Error ? cause.message : 'Could not continue the interview.')
    } finally {
      if (id === runId.current) setStreaming(false)
    }
  }, [agent, guardCurrent, model, repo.id, repo.path, repoSlug])

  const reset = useCallback((startImmediately: boolean): void => {
    abortRef.current?.abort()
    runId.current += 1
    const seed = seedIntent.trim()
    const initial: ChatTurn[] = seed ? [{ role: 'user', content: seed }] : []
    const initialIntake = socraticIntake()
    messagesRef.current = initial
    intakeRef.current = initialIntake
    pendingReply.current = initial.length > 0
    setMessages(initial)
    setIntake(initialIntake)
    setAnswer('')
    setLiveReply('')
    setStreaming(false)
    setExtracting(false)
    setContextMissing(false)
    setError(null)
    if (startImmediately && initial.length > 0) void askNext(initial, initialIntake)
  }, [askNext, seedIntent])

  useEffect(() => {
    if (!open) return
    if (activeSession.current !== sessionKey) {
      activeSession.current = sessionKey
      reset(true)
    } else if (pendingReply.current && !streaming) {
      void askNext(messagesRef.current, intakeRef.current)
    }
  }, [askNext, open, reset, sessionKey, streaming])

  useEffect(() => () => {
    abortRef.current?.abort()
    runId.current += 1
  }, [])

  const handleOpenChange = (next: boolean): void => {
    if (!next) {
      abortRef.current?.abort()
      runId.current += 1
      setStreaming(false)
    }
    onOpenChange(next)
  }

  const submit = (): void => {
    const content = answer.trim()
    if (!content || streaming || isSocraticComplete(intake)) return
    const history = [...messagesRef.current, { role: 'user' as const, content }]
    messagesRef.current = history
    setMessages(history)
    setAnswer('')
    void askNext(history, intakeRef.current)
  }

  const review = async (): Promise<void> => {
    if (extracting || !isSocraticComplete(intake) || !guardCurrent()) return
    setExtracting(true)
    setError(null)
    const controller = new AbortController()
    abortRef.current = controller
    try {
      const draft = await previewIssueSpec(messagesRef.current, {
        workdir: repo.path,
        repoId: repo.id,
        repoSlug,
        agent,
        model,
        signal: controller.signal
      })
      if (!guardCurrent()) return
      onApplyDraft(draft)
      pendingReply.current = false
      activeSession.current = null
      setExtracting(false)
      handleOpenChange(false)
    } catch (cause) {
      if (!controller.signal.aborted) {
        setError(cause instanceof Error ? cause.message : 'Could not shape the issue.')
      }
    } finally {
      if (!controller.signal.aborted) setExtracting(false)
    }
  }

  const complete = isSocraticComplete(intake)

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="flex h-[min(720px,82vh)] max-w-2xl flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl">
        <DialogHeader className="border-b border-border px-5 py-4 pr-12">
          <div className="flex items-center gap-2">
            <span className="flex size-7 items-center justify-center rounded-md bg-primary/10 text-primary">
              <Sparkles className="size-3.5" />
            </span>
            <div>
              <DialogTitle className="text-[15px]">Shape into spec</DialogTitle>
              <DialogDescription className="mt-0.5 text-[11.5px]">
                A focused conversation that returns one issue for review.
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="border-b border-border px-5 py-3">
          <div className="flex items-center gap-1.5">
            {STAGES.map((label, index) => {
              const stage = index + 1
              const active = intake.stage === stage && !complete
              const done = complete || intake.stage > stage
              return (
                <React.Fragment key={label}>
                  <span className={done || active ? 'text-[10.5px] text-foreground' : 'text-[10.5px] text-muted-foreground/60'}>
                    {label}
                  </span>
                  {index < STAGES.length - 1 ? (
                    <span className={done ? 'h-px min-w-3 flex-1 bg-primary/50' : 'h-px min-w-3 flex-1 bg-border'} />
                  ) : null}
                </React.Fragment>
              )
            })}
          </div>
        </div>

        <div className="scrollbar-sleek flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto bg-secondary/20 px-5 py-4">
          {messages.map((message, index) => (
            <div
              key={`${message.role}-${index}`}
              className={message.role === 'user'
                ? 'ml-auto max-w-[82%] rounded-xl rounded-br-sm bg-primary px-3 py-2 text-[12.5px] leading-relaxed text-primary-foreground'
                : 'max-w-[88%] whitespace-pre-wrap text-[12.5px] leading-relaxed text-foreground'}
            >
              {message.content}
            </div>
          ))}
          {streaming ? (
            <div className="max-w-[88%] whitespace-pre-wrap text-[12.5px] leading-relaxed text-foreground">
              {liveReply || <Loader2 className="size-4 animate-spin text-muted-foreground" />}
            </div>
          ) : null}
          {contextMissing ? (
            <span className="text-[10.5px] text-amber-600 dark:text-amber-400">
              Repo context is unavailable; answers will stay intentionally generic.
            </span>
          ) : null}
          {error ? <span className="text-[11px] text-destructive">{error}</span> : null}
        </div>

        <div className="border-t border-border bg-background px-5 py-4">
          {complete ? (
            <div className="flex items-center justify-between gap-3">
              <div>
                <p className="text-[12px] font-medium text-foreground">Ready to shape</p>
                <p className="text-[11px] text-muted-foreground">You can still review and edit everything before filing.</p>
              </div>
              <button
                type="button"
                onClick={() => void review()}
                disabled={extracting}
                className="inline-flex items-center gap-1.5 rounded-full bg-primary px-3.5 py-1.5 text-[12px] font-medium text-primary-foreground disabled:opacity-50"
              >
                {extracting ? <Loader2 className="size-3.5 animate-spin" /> : null}
                {extracting ? 'Shaping…' : 'Review issue'}
                {!extracting ? <ArrowRight className="size-3.5" /> : null}
              </button>
            </div>
          ) : (
            <div className="flex items-end gap-2">
              <textarea
                value={answer}
                onChange={(event) => setAnswer(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && !event.shiftKey) {
                    event.preventDefault()
                    submit()
                  }
                }}
                rows={2}
                disabled={streaming}
                placeholder={streaming ? 'Thinking…' : 'Answer briefly…'}
                className="min-h-[54px] flex-1 resize-none rounded-md border border-input bg-secondary px-2.5 py-2 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring disabled:opacity-60"
              />
              <button
                type="button"
                onClick={submit}
                disabled={!answer.trim() || streaming}
                className="flex size-8 items-center justify-center rounded-md bg-primary text-primary-foreground disabled:opacity-40"
                aria-label="Send answer"
              >
                {streaming ? <Loader2 className="size-3.5 animate-spin" /> : <ArrowRight className="size-3.5" />}
              </button>
            </div>
          )}
          <button
            type="button"
            onClick={() => reset(true)}
            disabled={streaming || extracting}
            className="mt-2 inline-flex items-center gap-1 text-[10.5px] text-muted-foreground transition-colors hover:text-foreground disabled:opacity-40"
          >
            <RotateCcw className="size-3" />
            Start over
          </button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
