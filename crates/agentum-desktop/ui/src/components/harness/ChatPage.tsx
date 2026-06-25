// Chat — a real conversational front door to the Spec→tickets pipeline (#48).
// Describe a feature in plain words and the server-side Socratic interviewer
// (`POST /api/chat`) asks a few clarifying questions, then proposes a task
// breakdown the user can file into GitHub or Linear (toggle, when Linear is
// connected). The conversation streams turns to/from the embedded server via
// chat-client; the "Create issues" button is the only mutation here — review and
// start-task happen on the Board.
import { type FormEvent, type KeyboardEvent, useCallback, useEffect, useRef, useState } from 'react'
import { Columns3, Github, Loader2, MessagesSquare, Send, Sparkles } from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { DrillInHeader } from '@/components/nav/DrillInHeader'
import { LinearIcon } from '@/components/icons/LinearIcon'
import CommentMarkdown from '@/components/sidebar/CommentMarkdown'
import { type ChatTurn, createIssuesFromChat, type IssueProvider, sendChat } from '@/runtime/chat-client'

export default function ChatPage() {
  const repos = useAppStore((s) => s.repos)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const linearStatus = useAppStore((s) => s.linearStatus)
  const linearStatusChecked = useAppStore((s) => s.linearStatusChecked)
  const checkLinearConnection = useAppStore((s) => s.checkLinearConnection)

  const [messages, setMessages] = useState<ChatTurn[]>([])
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Which tracker the "Create issues" button files into. Only meaningful when
  // Linear is connected; GitHub is the default and the sole option otherwise.
  const [provider, setProvider] = useState<IssueProvider>('github')

  // The "Create issues" affordance only appears once the interviewer has
  // proposed something — i.e. there's at least one assistant turn to mine.
  const hasAssistantReply = messages.some((m) => m.role === 'assistant')

  // Lazily discover whether Linear is connected (same check the sidebar +
  // integrations panes use) so we can offer it as a target. GitHub stays the
  // default; the toggle only appears when Linear is actually connected.
  useEffect(() => {
    if (!linearStatusChecked) void checkLinearConnection()
  }, [linearStatusChecked, checkLinearConnection])

  const linearConnected = linearStatus.connected === true
  // When Linear isn't connected, GitHub is the only valid target regardless of
  // any stale `provider` state.
  const effectiveProvider: IssueProvider = linearConnected ? provider : 'github'

  // Auto-scroll to the newest message (or the thinking indicator) whenever the
  // transcript grows or the busy state flips.
  const bottomRef = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' })
  }, [messages, busy])

  const submit = useCallback(
    async (e?: FormEvent) => {
      e?.preventDefault()
      const text = draft.trim()
      if (!text || busy) return

      const userTurn: ChatTurn = { role: 'user', content: text }
      const history = [...messages, userTurn]
      // Optimistically render the user's turn and clear the composer.
      setMessages(history)
      setDraft('')
      setBusy(true)
      setError(null)
      try {
        // Ground the interviewer in the active project's workdir so it can read
        // the repo when proposing the task breakdown. repoSlug is omitted — the
        // server resolves the tracker target on its own.
        const reply = await sendChat(history, { workdir: repos[0]?.path })
        setMessages((prev) => [...prev, { role: 'assistant', content: reply }])
      } catch (e2) {
        // Surface the server's specific reason ("Sign in to Claude…", llm_failed)
        // and DON'T append a bad assistant turn — the user's turn stays so they
        // can fix the cause and retry.
        setError(e2 instanceof Error ? e2.message : String(e2))
      } finally {
        setBusy(false)
      }
    },
    [draft, busy, messages, repos]
  )

  // Enter submits; Shift+Enter inserts a newline.
  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        void submit()
      }
    },
    [submit]
  )

  // Close the loop: mine the agreed task breakdown out of the transcript and
  // file each task into the chosen tracker (GitHub or Linear), then append a
  // summary turn linking them.
  const createIssues = useCallback(async () => {
    if (busy || creating || messages.length === 0) return
    setCreating(true)
    setError(null)
    try {
      const result = await createIssuesFromChat(messages, {
        workdir: repos[0]?.path,
        provider: effectiveProvider
      })
      // "where" reads naturally for either tracker: a GitHub repo slug, or "Linear".
      const where =
        result.provider === 'linear' ? 'Linear' : result.repo ? `\`${result.repo}\`` : 'GitHub'
      const lines: string[] = []
      if (result.created.length > 0) {
        const n = result.created.length
        lines.push(`Created ${n} issue${n === 1 ? '' : 's'} in ${where}:`)
        for (const c of result.created) {
          // Linear sends a human identifier (ENG-42) worth showing; GitHub omits it.
          const label = c.id ? `${c.id} — ${c.title}` : c.title
          lines.push(c.url ? `- [${label}](${c.url})` : `- ${label}`)
        }
      } else {
        lines.push(`No issues were created in ${where}.`)
      }
      if (result.failed.length > 0) {
        lines.push('')
        lines.push(`${result.failed.length} could not be created:`)
        for (const f of result.failed) lines.push(`- ${f.title} — ${f.error}`)
      }
      setMessages((prev) => [...prev, { role: 'assistant', content: lines.join('\n') }])
    } catch (e2) {
      setError(e2 instanceof Error ? e2.message : String(e2))
    } finally {
      setCreating(false)
    }
  }, [busy, creating, messages, repos, effectiveProvider])

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <DrillInHeader
        icon={MessagesSquare}
        title="Chat"
        description="Describe a feature — I'll ask a few questions, then propose the tasks to create"
        actions={
          <button
            type="button"
            onClick={() => setActiveView('tasks')}
            className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[12.5px] font-medium hover:border-foreground/30 hover:bg-accent"
          >
            <Columns3 className="size-3.5" /> Open Board
          </button>
        }
      />

      {/* transcript */}
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-6">
        <div className="mx-auto flex max-w-[720px] flex-col gap-4">
          {messages.length === 0 && !busy ? (
            <div className="rounded-lg border border-dashed border-border p-8 text-center text-muted-foreground">
              <MessagesSquare className="mx-auto mb-3 size-6 opacity-60" />
              <div className="text-sm">
                Describe a feature — I'll ask a few questions, then propose the tasks to create.
              </div>
            </div>
          ) : null}

          {messages.map((m, i) => (
            <Bubble key={i} turn={m} />
          ))}

          {busy ? (
            <div className="flex items-center gap-2.5 text-muted-foreground">
              <div className="grid size-7 flex-none place-items-center rounded-full border border-primary/40 bg-primary/10">
                <Sparkles className="size-3.5 text-primary" />
              </div>
              <span className="inline-flex items-center gap-1.5 font-mono text-[12px]">
                <Loader2 className="size-3.5 animate-spin" /> thinking…
              </span>
            </div>
          ) : null}

          {error ? (
            <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-[12px] text-red-400">
              {error}
            </div>
          ) : null}

          <div ref={bottomRef} />
        </div>
      </div>

      {/* composer */}
      <form onSubmit={submit} className="flex-none border-t border-border px-5 pb-4.5 pt-3">
        {hasAssistantReply ? (
          <div className="mx-auto mb-2.5 flex max-w-[720px] items-center justify-end gap-2">
            {/* Tracker toggle — only when Linear is connected; GitHub-only users
                keep the unchanged single-button experience. */}
            {linearConnected ? (
              <div
                role="group"
                aria-label="Issue tracker"
                className="inline-flex overflow-hidden rounded-md border border-border"
              >
                <button
                  type="button"
                  onClick={() => setProvider('github')}
                  disabled={creating}
                  className={cn(
                    'inline-flex items-center gap-1.5 px-2.5 py-1 text-[12.5px] font-medium hover:bg-accent disabled:opacity-40',
                    effectiveProvider === 'github'
                      ? 'bg-accent text-foreground'
                      : 'text-muted-foreground'
                  )}
                >
                  <Github className="size-3.5" /> GitHub
                </button>
                <button
                  type="button"
                  onClick={() => setProvider('linear')}
                  disabled={creating}
                  className={cn(
                    'inline-flex items-center gap-1.5 border-l border-border px-2.5 py-1 text-[12.5px] font-medium hover:bg-accent disabled:opacity-40',
                    effectiveProvider === 'linear'
                      ? 'bg-accent text-foreground'
                      : 'text-muted-foreground'
                  )}
                >
                  <LinearIcon className="size-3.5" /> Linear
                </button>
              </div>
            ) : null}
            <button
              type="button"
              onClick={() => void createIssues()}
              disabled={busy || creating || messages.length === 0}
              className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[12.5px] font-medium hover:border-foreground/30 hover:bg-accent disabled:opacity-40"
            >
              {creating ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : effectiveProvider === 'linear' ? (
                <LinearIcon className="size-3.5" />
              ) : (
                <Github className="size-3.5" />
              )}
              {creating
                ? 'Creating issues…'
                : `Create ${effectiveProvider === 'linear' ? 'Linear' : 'GitHub'} issues`}
            </button>
          </div>
        ) : null}
        <div className="mx-auto flex max-w-[720px] items-end gap-2.5 rounded-lg border border-border bg-card px-3 py-2.5">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKeyDown}
            rows={1}
            placeholder='Try "Add a CSV export to the board"…  (Enter to send · Shift+Enter for newline)'
            className="max-h-40 flex-1 resize-none bg-transparent py-1 text-[14px] leading-relaxed text-foreground placeholder:text-muted-foreground focus:outline-none"
          />
          <button
            type="submit"
            disabled={busy || !draft.trim()}
            className="inline-flex size-8 flex-none items-center justify-center rounded-md bg-primary text-primary-foreground hover:opacity-85 disabled:opacity-40"
            aria-label="Send"
          >
            {busy ? <Loader2 className="size-4 animate-spin" /> : <Send className="size-4" />}
          </button>
        </div>
      </form>
    </div>
  )
}

/** One chat turn: user bubbles are accent + right-aligned, assistant bubbles are
 *  muted + left-aligned and render markdown (the task-breakdown bullet lists). */
function Bubble({ turn }: { turn: ChatTurn }) {
  const isUser = turn.role === 'user'
  return (
    <div className={cn('flex items-start gap-3', isUser && 'flex-row-reverse')}>
      <div
        className={cn(
          'grid size-7 flex-none place-items-center rounded-full border',
          isUser ? 'border-border bg-card' : 'border-primary/40 bg-primary/10'
        )}
      >
        {isUser ? (
          <span className="font-mono text-[10px]">you</span>
        ) : (
          <Sparkles className="size-3.5 text-primary" />
        )}
      </div>
      <div className={cn('flex min-w-0 max-w-[80%] flex-col', isUser && 'items-end')}>
        <div className="mb-1.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
          {isUser ? 'you' : 'agentum'}
        </div>
        {isUser ? (
          <div className="whitespace-pre-wrap rounded-lg border border-primary/30 bg-primary/10 px-4 py-3 text-[14px] leading-relaxed text-foreground">
            {turn.content}
          </div>
        ) : (
          <CommentMarkdown
            content={turn.content}
            variant="compact"
            className="rounded-lg border border-border bg-card px-4 py-3 text-[14px] leading-relaxed"
          />
        )}
      </div>
    </div>
  )
}
