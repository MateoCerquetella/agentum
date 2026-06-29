// Chat — a real conversational front door to the Spec→tickets pipeline (#48).
// Describe a feature in plain words and the server-side Socratic interviewer
// (`POST /api/chat/stream`) asks a few clarifying questions, then proposes a task
// breakdown the user can file as GitHub issues. The target repo is INFERRED from
// the selected workspace (a picker in the toolbar) — that same workspace grounds
// the interview — so there's no manual owner/repo entry. The reply streams in
// token-by-token; an optional extended-thinking trace is shown above each answer.
// Conversations are kept in local history (sidebar), and the workspace + model +
// thinking are user-pickable. The "Create issues" button is the only mutation
// here — review and start-task happen on the Board.
import { type FormEvent, type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Brain,
  Check,
  ChevronDown,
  Columns3,
  Cpu,
  FolderGit2,
  Github,
  Loader2,
  MessagesSquare,
  Plus,
  Send,
  Square,
  Trash2,
  User
} from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import type { Repo } from '@/shared/types'
import { DrillInHeader } from '@/components/nav/DrillInHeader'
import { AgentumMark } from '@/components/icons/AgentumMark'
import CommentMarkdown from '@/components/sidebar/CommentMarkdown'
import {
  CHAT_MODELS,
  createIssuesFromChat,
  DEFAULT_CHAT_MODEL,
  resolveChatModel,
  streamChat
} from '@/runtime/chat-client'
import {
  type Conversation,
  loadConversations,
  newConversationId,
  saveConversations,
  type StoredTurn,
  titleFromMessages,
  upsertConversation
} from '@/runtime/chat-history'

// Picker defaults persist across restarts so the user doesn't re-pick every time
// (same client-persistence pattern as the planner tool / profiles).
const MODEL_KEY = 'agentum.chat.model'
const THINKING_KEY = 'agentum.chat.thinking'
const WORKSPACE_KEY = 'agentum.chat.workspace'

function readStoredModel(): string {
  try {
    return localStorage.getItem(MODEL_KEY) || DEFAULT_CHAT_MODEL
  } catch {
    return DEFAULT_CHAT_MODEL
  }
}
function readStoredThinking(): boolean {
  try {
    return localStorage.getItem(THINKING_KEY) === '1'
  } catch {
    return false
  }
}

export default function ChatPage() {
  const repos = useAppStore((s) => s.repos)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const setActiveView = useAppStore((s) => s.setActiveView)

  const [conversations, setConversations] = useState<Conversation[]>(() => loadConversations())
  const [activeId, setActiveId] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Model + extended-thinking picker (persisted defaults; also remembered per
  // conversation so reopening one restores what it last ran with).
  const [model, setModel] = useState<string>(readStoredModel)
  const [thinking, setThinking] = useState<boolean>(readStoredThinking)
  useEffect(() => {
    try {
      localStorage.setItem(MODEL_KEY, model)
    } catch {
      /* storage may be unavailable — picker still works this session */
    }
  }, [model])
  useEffect(() => {
    try {
      localStorage.setItem(THINKING_KEY, thinking ? '1' : '0')
    } catch {
      /* ignore */
    }
  }, [thinking])

  // Which workspace grounds the interview AND receives the issues. The selected
  // project's GitHub repo is the (inferred) issue target — there is no manual
  // owner/repo entry. Seeded from the app's active project; persisted.
  const [workspaceId, setWorkspaceId] = useState<string | null>(() => {
    try {
      return localStorage.getItem(WORKSPACE_KEY)
    } catch {
      return null
    }
  })
  // Validate/seed the selection once projects have loaded. While `repos` is empty
  // (store not yet hydrated) we MUST NOT run: the stored id would look stale
  // against the empty list and get reset to repos[0] on every launch, destroying
  // the user's saved choice. Once hydrated: keep a still-valid selection, else
  // fall back to the active project, then the first one.
  useEffect(() => {
    if (repos.length === 0) return
    setWorkspaceId((cur) => {
      if (cur && repos.some((r) => r.id === cur)) return cur
      return activeRepoId ?? repos[0]?.id ?? null
    })
  }, [repos, activeRepoId])
  useEffect(() => {
    try {
      if (workspaceId) localStorage.setItem(WORKSPACE_KEY, workspaceId)
    } catch {
      /* storage may be unavailable — selection still works this session */
    }
  }, [workspaceId])
  const workspace = useMemo(
    () => repos.find((r) => r.id === workspaceId) ?? null,
    [repos, workspaceId]
  )

  // The in-flight stream's abort handle, so the Stop button can cancel it.
  const abortRef = useRef<AbortController | null>(null)
  const textareaRef = useRef<HTMLTextAreaElement | null>(null)

  // Persist history shortly after the last change — coalesces the per-token state
  // updates of a streaming reply into a single localStorage write.
  useEffect(() => {
    const t = setTimeout(() => saveConversations(conversations), 400)
    return () => clearTimeout(t)
  }, [conversations])

  const active = useMemo(
    () => conversations.find((c) => c.id === activeId) ?? null,
    [conversations, activeId]
  )
  const messages = active?.messages ?? []
  const hasAssistantReply = messages.some((m) => m.role === 'assistant' && m.content.trim().length > 0)

  // Auto-follow the newest content. Instant while streaming (token follow),
  // smooth otherwise.
  const bottomRef = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: busy ? 'auto' : 'smooth', block: 'end' })
  }, [conversations, busy])

  // --- conversation state helpers (immutable, no re-sort mid-stream) ---

  const updateLastAssistant = useCallback(
    (id: string, patch: { content?: string; thinking?: string }) => {
      setConversations((prev) =>
        prev.map((c) => {
          if (c.id !== id) return c
          const msgs = c.messages.slice()
          const last = msgs[msgs.length - 1]
          if (!last || last.role !== 'assistant') return c
          msgs[msgs.length - 1] = { ...last, ...patch }
          return { ...c, messages: msgs }
        })
      )
    },
    []
  )

  const appendAssistant = useCallback((id: string, content: string) => {
    setConversations((prev) =>
      prev.map((c) =>
        c.id === id
          ? { ...c, messages: [...c.messages, { role: 'assistant', content }], updatedAt: Date.now() }
          : c
      )
    )
  }, [])

  const startNewChat = useCallback(() => {
    abortRef.current?.abort()
    setActiveId(null)
    setDraft('')
    setError(null)
    // Default the picker back to the stored preference for a fresh chat.
    setModel(readStoredModel())
    setThinking(readStoredThinking())
    textareaRef.current?.focus()
  }, [])

  const selectConversation = useCallback(
    (c: Conversation) => {
      if (busy) return
      setActiveId(c.id)
      setError(null)
      setModel(c.model || DEFAULT_CHAT_MODEL)
      setThinking(!!c.thinking)
    },
    [busy]
  )

  const removeConversation = useCallback((c: Conversation) => {
    const ok = window.confirm(`Delete "${c.title}"? This can't be undone.`)
    if (!ok) return
    setConversations((prev) => prev.filter((x) => x.id !== c.id))
    setActiveId((cur) => (cur === c.id ? null : cur))
  }, [])

  const submit = useCallback(
    async (e?: FormEvent) => {
      e?.preventDefault()
      const text = draft.trim()
      if (!text || busy) return
      setError(null)

      const convoId = activeId ?? newConversationId()
      const isNew = activeId == null
      const now = Date.now()
      const userTurn: StoredTurn = { role: 'user', content: text }

      // Wire history = prior turns + this user turn (the streamed-into placeholder
      // is excluded). Computed from current state before the optimistic update.
      const prior = conversations.find((c) => c.id === convoId)?.messages ?? []
      const history = [...prior, userTurn]

      // Optimistically render the user turn + an empty assistant turn to stream into.
      setConversations((prev) => {
        if (isNew) {
          const convo: Conversation = {
            id: convoId,
            title: titleFromMessages([userTurn]),
            messages: [userTurn, { role: 'assistant', content: '', thinking: '' }],
            model,
            thinking,
            createdAt: now,
            updatedAt: now
          }
          return upsertConversation(prev, convo)
        }
        return prev.map((c) =>
          c.id === convoId
            ? {
                ...c,
                messages: [...c.messages, userTurn, { role: 'assistant', content: '', thinking: '' }],
                model,
                thinking,
                updatedAt: now
              }
            : c
        )
      })
      setActiveId(convoId)
      setDraft('')
      setBusy(true)

      const ac = new AbortController()
      abortRef.current = ac
      let content = ''
      let reasoning = ''
      try {
        await streamChat(history, {
          workdir: workspace?.path,
          model,
          thinking,
          signal: ac.signal,
          onDelta: (d) => {
            if (d.type === 'text') content += d.text
            else if (d.type === 'thinking') reasoning += d.text
            updateLastAssistant(convoId, { content, thinking: reasoning })
          }
        })
        // Bump updatedAt so the conversation floats to the top of the history list.
        setConversations((prev) =>
          prev.map((c) => (c.id === convoId ? { ...c, updatedAt: Date.now() } : c))
        )
      } catch (e2) {
        // Aborted (Stop) keeps whatever streamed; a real failure surfaces the
        // server's specific reason. Either way, drop a still-empty assistant turn
        // so we never leave a blank bubble.
        if (!ac.signal.aborted) {
          setError(e2 instanceof Error ? e2.message : String(e2))
        }
        setConversations((prev) =>
          prev.map((c) => {
            if (c.id !== convoId) return c
            const msgs = c.messages.slice()
            const last = msgs[msgs.length - 1]
            if (last && last.role === 'assistant' && !last.content && !last.thinking) msgs.pop()
            return { ...c, messages: msgs }
          })
        )
      } finally {
        if (abortRef.current === ac) abortRef.current = null
        setBusy(false)
      }
    },
    [draft, busy, activeId, conversations, model, thinking, workspace, updateLastAssistant]
  )

  const stop = useCallback(() => abortRef.current?.abort(), [])

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

  // Mine the agreed task breakdown out of the transcript and file each task into
  // the chosen tracker, then append a summary turn linking the created issues.
  const createIssues = useCallback(async () => {
    if (busy || creating || !active || messages.length === 0) return
    const convoId = active.id
    setCreating(true)
    setError(null)
    try {
      const result = await createIssuesFromChat(
        messages.map((m) => ({ role: m.role, content: m.content })),
        // GitHub-only: the server infers the repo from the workspace's `origin`
        // (no manual owner/repo). The workspace path is the sole hint it needs.
        { workdir: workspace?.path }
      )
      const where = result.repo ? `\`${result.repo}\`` : workspace ? workspace.displayName : 'GitHub'
      const lines: string[] = []
      if (result.created.length > 0) {
        const n = result.created.length
        lines.push(`Created ${n} issue${n === 1 ? '' : 's'} in ${where}:`)
        for (const c of result.created) {
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
      appendAssistant(convoId, lines.join('\n'))
    } catch (e2) {
      setError(e2 instanceof Error ? e2.message : String(e2))
    } finally {
      setCreating(false)
    }
  }, [busy, creating, active, messages, workspace, appendAssistant])

  const useExample = useCallback((text: string) => {
    setDraft(text)
    textareaRef.current?.focus()
  }, [])

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

      <div className="flex min-h-0 flex-1">
        {/* ---- conversation history ---- */}
        <aside className="flex w-60 flex-none flex-col border-r border-border bg-sidebar/60">
          <div className="p-3">
            <button
              type="button"
              onClick={startNewChat}
              className="flex w-full items-center gap-2 rounded-md border border-border bg-card px-3 py-2 text-[13px] font-medium transition-colors hover:border-foreground/30 hover:bg-accent"
            >
              <Plus className="size-3.5" /> New chat
            </button>
          </div>
          <div className="px-3.5 pb-1.5 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
            History
          </div>
          <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-3">
            {conversations.length === 0 ? (
              <div className="px-3 py-2 text-[12px] text-muted-foreground">No chats yet.</div>
            ) : (
              conversations.map((c) => {
                const isActive = c.id === activeId
                return (
                  <div key={c.id} className="group relative">
                    <button
                      type="button"
                      onClick={() => selectConversation(c)}
                      className={cn(
                        'flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 pr-8 text-left transition-colors',
                        isActive ? 'bg-accent' : 'hover:bg-foreground/5'
                      )}
                    >
                      <span className="truncate text-[13px]">{c.title}</span>
                      <span className="flex items-center gap-1.5 truncate font-mono text-[10px] text-muted-foreground">
                        <span>{timeAgo(c.updatedAt)}</span>
                        <span aria-hidden>·</span>
                        <span className="truncate">{shortModel(c.model)}</span>
                        {c.thinking ? (
                          <Brain className="size-3 text-primary/70" aria-label="thinking" />
                        ) : null}
                      </span>
                    </button>
                    <button
                      type="button"
                      onClick={() => removeConversation(c)}
                      aria-label={`Delete chat: ${c.title}`}
                      title="Delete chat"
                      className="absolute right-1.5 top-1.5 hidden rounded p-1 text-muted-foreground hover:bg-foreground/10 hover:text-red-400 group-hover:block"
                    >
                      <Trash2 className="size-3.5" />
                    </button>
                  </div>
                )
              })
            )}
          </div>
        </aside>

        {/* ---- chat column ---- */}
        <div className="flex min-h-0 flex-1 flex-col">
          {/* workspace + model + thinking toolbar */}
          <div className="flex h-11 flex-none items-center gap-2 border-b border-border px-4">
            <WorkspacePicker
              repos={repos}
              workspaceId={workspaceId}
              onChange={setWorkspaceId}
              disabled={busy}
            />
            <ModelPicker model={model} onChange={setModel} disabled={busy} />
            <ThinkingToggle on={thinking} onToggle={setThinking} disabled={busy} />
            <span className="ml-auto hidden font-mono text-[11px] text-muted-foreground md:inline">
              agentum · spec → issues
            </span>
          </div>

          {/* transcript */}
          <div className="min-h-0 flex-1 overflow-y-auto px-5 py-6">
            <div className="mx-auto flex max-w-[760px] flex-col gap-5">
              {messages.length === 0 && !busy ? (
                <EmptyState onPick={useExample} />
              ) : (
                messages.map((m, i) => (
                  <Bubble
                    key={i}
                    turn={m}
                    streaming={busy && i === messages.length - 1 && m.role === 'assistant'}
                  />
                ))
              )}

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
              <div className="mx-auto mb-2.5 flex max-w-[760px] flex-col gap-1.5">
                <div className="flex flex-wrap items-center justify-end gap-2">
                  <button
                    type="button"
                    onClick={() => void createIssues()}
                    disabled={busy || creating || messages.length === 0 || !workspace}
                    className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[12.5px] font-medium hover:border-foreground/30 hover:bg-accent disabled:opacity-40"
                  >
                    {creating ? (
                      <Loader2 className="size-3.5 animate-spin" />
                    ) : (
                      <Github className="size-3.5" />
                    )}
                    {creating
                      ? 'Creating issues…'
                      : workspace
                        ? `Create issues in ${workspace.displayName}`
                        : 'Create issues'}
                  </button>
                </div>
                <div className="text-right text-[11px] text-muted-foreground">
                  {workspace
                    ? `Files GitHub issues into ${workspace.displayName} — inferred from the workspace.`
                    : 'Pick a workspace above to file issues into.'}
                </div>
              </div>
            ) : null}
            <div className="mx-auto flex max-w-[760px] items-end gap-2.5 rounded-xl border border-border bg-card px-3 py-2.5 focus-within:border-foreground/30">
              <textarea
                ref={textareaRef}
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={onKeyDown}
                rows={1}
                placeholder='Try "Add a CSV export to the board"…  (Enter to send · Shift+Enter for newline)'
                className="max-h-40 flex-1 resize-none bg-transparent py-1 text-[14px] leading-relaxed text-foreground placeholder:text-muted-foreground focus:outline-none"
              />
              {busy ? (
                <button
                  type="button"
                  onClick={stop}
                  className="inline-flex size-8 flex-none items-center justify-center rounded-lg border border-border text-foreground hover:bg-accent"
                  aria-label="Stop generating"
                  title="Stop generating"
                >
                  <Square className="size-3.5 fill-current" />
                </button>
              ) : (
                <button
                  type="submit"
                  disabled={!draft.trim()}
                  className="inline-flex size-8 flex-none items-center justify-center rounded-lg bg-primary text-primary-foreground transition-opacity hover:opacity-85 disabled:opacity-40"
                  aria-label="Send"
                >
                  <Send className="size-4" />
                </button>
              )}
            </div>
          </form>
        </div>
      </div>
    </div>
  )
}

/** The assistant mark — the agentum brand glyph (the stacked-square "A", from
 *  `resources/logo.svg`) in white on a gradient tile, so the chat reads as
 *  agentum rather than a generic AI. */
function AssistantAvatar({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        'relative grid flex-none place-items-center overflow-hidden rounded-[7px] bg-gradient-to-br from-indigo-500 via-violet-500 to-fuchsia-500 text-white shadow-sm ring-1 ring-inset ring-white/15',
        className
      )}
      aria-hidden
    >
      <AgentumMark className="size-[72%]" />
    </span>
  )
}

/** Workspace dropdown — picks which open project grounds the interview and (since
 *  the GitHub repo is inferred from it) receives the issues. Empty when no project
 *  is open; remote projects are tagged (Chat files issues against the LOCAL repo,
 *  so a remote selection grounds context but can't infer a local origin). */
function WorkspacePicker({
  repos,
  workspaceId,
  onChange,
  disabled
}: {
  repos: Repo[]
  workspaceId: string | null
  onChange: (id: string) => void
  disabled?: boolean
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener('mousedown', onDown)
    return () => window.removeEventListener('mousedown', onDown)
  }, [open])
  const current = repos.find((r) => r.id === workspaceId) ?? null
  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        disabled={disabled || repos.length === 0}
        onClick={() => setOpen((o) => !o)}
        className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[12.5px] font-medium hover:border-foreground/30 hover:bg-accent disabled:opacity-50"
        aria-haspopup="listbox"
        aria-expanded={open}
        title="Workspace the chat is grounded in — and where issues are filed"
      >
        <FolderGit2 className="size-3.5 text-muted-foreground" />
        <span className="max-w-[12rem] truncate">
          {current ? current.displayName : 'No workspace'}
        </span>
        <ChevronDown
          className={cn('size-3.5 text-muted-foreground transition-transform', open && 'rotate-180')}
        />
      </button>
      {open ? (
        <div
          role="listbox"
          className="absolute left-0 top-full z-30 mt-1 w-72 rounded-lg border border-border bg-card p-1 shadow-lg"
        >
          {repos.length === 0 ? (
            <div className="px-2 py-1.5 text-[12px] text-muted-foreground">
              No projects open — add one from the sidebar.
            </div>
          ) : (
            repos.map((r) => {
              const sel = r.id === workspaceId
              const remote = !!(r.connectionId || r.hostId)
              return (
                <button
                  key={r.id}
                  type="button"
                  role="option"
                  aria-selected={sel}
                  onClick={() => {
                    onChange(r.id)
                    setOpen(false)
                  }}
                  className={cn(
                    'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent',
                    sel && 'bg-accent/60'
                  )}
                >
                  <FolderGit2 className="size-3.5 flex-none text-muted-foreground" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px] font-medium text-foreground">
                      {r.displayName}
                    </div>
                    <div className="truncate font-mono text-[10.5px] text-muted-foreground">
                      {remote ? 'remote · ' : ''}
                      {r.path}
                    </div>
                  </div>
                  {sel ? <Check className="size-4 flex-none text-primary" /> : null}
                </button>
              )
            })
          )}
        </div>
      ) : null}
    </div>
  )
}

/** Compact model dropdown (popover) — name + one-line blurb + check. */
function ModelPicker({
  model,
  onChange,
  disabled
}: {
  model: string
  onChange: (id: string) => void
  disabled?: boolean
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener('mousedown', onDown)
    return () => window.removeEventListener('mousedown', onDown)
  }, [open])
  const current = resolveChatModel(model)
  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[12.5px] font-medium hover:border-foreground/30 hover:bg-accent disabled:opacity-50"
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <Cpu className="size-3.5 text-muted-foreground" />
        <span>{current.label}</span>
        <ChevronDown
          className={cn('size-3.5 text-muted-foreground transition-transform', open && 'rotate-180')}
        />
      </button>
      {open ? (
        <div
          role="listbox"
          className="absolute left-0 top-full z-30 mt-1 w-64 rounded-lg border border-border bg-card p-1 shadow-lg"
        >
          {CHAT_MODELS.map((m) => {
            const sel = m.id === current.id
            return (
              <button
                key={m.id}
                type="button"
                role="option"
                aria-selected={sel}
                onClick={() => {
                  onChange(m.id)
                  setOpen(false)
                }}
                className={cn(
                  'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent',
                  sel && 'bg-accent/60'
                )}
              >
                <div className="min-w-0 flex-1">
                  <div className="text-[13px] font-medium text-foreground">{m.label}</div>
                  <div className="truncate text-[11px] text-muted-foreground">{m.blurb}</div>
                </div>
                {sel ? <Check className="size-4 flex-none text-primary" /> : null}
              </button>
            )
          })}
        </div>
      ) : null}
    </div>
  )
}

/** Extended-thinking toggle pill. */
function ThinkingToggle({
  on,
  onToggle,
  disabled
}: {
  on: boolean
  onToggle: (next: boolean) => void
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={() => onToggle(!on)}
      aria-pressed={on}
      title="Extended thinking — the model reasons before answering (shown above each reply)"
      className={cn(
        'inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-[12.5px] font-medium transition-colors disabled:opacity-50',
        on
          ? 'border-primary/40 bg-primary/10 text-primary'
          : 'border-border text-muted-foreground hover:bg-accent'
      )}
    >
      <Brain className="size-3.5" /> Thinking
    </button>
  )
}

/** Collapsible reasoning trace shown above an assistant answer. Open while the
 *  reply streams, then auto-collapses — unless the user has toggled it. */
function Reasoning({ text, streaming }: { text: string; streaming: boolean }) {
  const [userOpen, setUserOpen] = useState<boolean | null>(null)
  const open = userOpen ?? streaming
  return (
    <div className="mb-2 overflow-hidden rounded-md border border-border/70 bg-foreground/[0.03]">
      <button
        type="button"
        onClick={() => setUserOpen(!open)}
        className="flex w-full items-center gap-1.5 px-2.5 py-1.5 text-[11px] font-medium text-muted-foreground hover:text-foreground"
      >
        <Brain className="size-3.5 text-primary/80" />
        <span>{streaming && !text ? 'Thinking…' : 'Reasoning'}</span>
        {streaming ? <Loader2 className="size-3 animate-spin" /> : null}
        <ChevronDown className={cn('ml-auto size-3.5 transition-transform', open && 'rotate-180')} />
      </button>
      {open && text ? (
        <div className="whitespace-pre-wrap border-t border-border/70 px-2.5 py-2 text-[12px] leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
          {text}
        </div>
      ) : null}
    </div>
  )
}

/** One chat turn. User turns are accent + right-aligned; assistant turns render
 *  markdown, with the optional reasoning trace above and a live indicator while
 *  streaming. */
function Bubble({ turn, streaming }: { turn: StoredTurn; streaming: boolean }) {
  const isUser = turn.role === 'user'
  return (
    <div className={cn('flex items-start gap-3', isUser && 'flex-row-reverse')}>
      {isUser ? (
        <div className="grid size-7 flex-none place-items-center rounded-full border border-border bg-card text-muted-foreground">
          <User className="size-3.5" />
        </div>
      ) : (
        <AssistantAvatar className="size-7" />
      )}
      <div className={cn('flex min-w-0 max-w-[82%] flex-col', isUser && 'items-end')}>
        <div className="mb-1.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
          {isUser ? 'you' : 'agentum'}
        </div>
        {isUser ? (
          <div className="whitespace-pre-wrap rounded-2xl rounded-tr-sm border border-primary/30 bg-primary/10 px-4 py-3 text-[14px] leading-relaxed text-foreground [overflow-wrap:anywhere]">
            {turn.content}
          </div>
        ) : (
          <div className="min-w-0">
            {turn.thinking ? <Reasoning text={turn.thinking} streaming={streaming} /> : null}
            {turn.content ? (
              <CommentMarkdown
                content={turn.content}
                variant="compact"
                className="rounded-2xl rounded-tl-sm border border-border bg-card px-4 py-3 text-[14px] leading-relaxed"
              />
            ) : streaming && !turn.thinking ? (
              <div className="inline-flex items-center gap-2 rounded-2xl rounded-tl-sm border border-border bg-card px-4 py-3 font-mono text-[12px] text-muted-foreground">
                <Loader2 className="size-3.5 animate-spin" /> thinking…
              </div>
            ) : null}
          </div>
        )}
      </div>
    </div>
  )
}

/** Empty conversation hero with a few one-click example prompts. */
function EmptyState({ onPick }: { onPick: (text: string) => void }) {
  const examples = [
    'Add a CSV export to the board',
    'Let users star a worktree to pin it to the top of the sidebar',
    'Add a global command palette shortcut to jump between sessions'
  ]
  return (
    <div className="mx-auto flex max-w-[560px] flex-col items-center px-4 py-10 text-center">
      <AssistantAvatar className="size-12" />
      <h2 className="mt-4 text-lg font-semibold tracking-tight text-foreground">Describe a feature</h2>
      <p className="mt-1.5 text-[13.5px] leading-relaxed text-muted-foreground">
        I'll ask a few sharp clarifying questions, then propose a task breakdown you can file as
        GitHub issues into your selected workspace.
      </p>
      <div className="mt-5 flex w-full flex-col gap-2">
        {examples.map((ex) => (
          <button
            key={ex}
            type="button"
            onClick={() => onPick(ex)}
            className="rounded-lg border border-border bg-card px-3.5 py-2.5 text-left text-[13px] text-foreground/90 transition-colors hover:border-foreground/30 hover:bg-accent"
          >
            {ex}
          </button>
        ))}
      </div>
    </div>
  )
}

/** A short, theme-free label for a model id used in the history list. */
function shortModel(id: string): string {
  return resolveChatModel(id).label.replace(/^Claude\s+/, '')
}

/** Compact relative time for the history list. */
function timeAgo(ts: number): string {
  const s = Math.max(0, Math.floor((Date.now() - ts) / 1000))
  if (s < 60) return 'just now'
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  const d = Math.floor(h / 24)
  if (d < 7) return `${d}d ago`
  const w = Math.floor(d / 7)
  if (w < 5) return `${w}w ago`
  const mo = Math.floor(d / 30)
  if (mo < 12) return `${mo}mo ago`
  return `${Math.floor(d / 365)}y ago`
}
