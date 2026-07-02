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
  ArrowUpRight,
  Brain,
  Check,
  ChevronDown,
  Columns3,
  Cpu,
  Eye,
  FolderGit2,
  Github,
  Loader2,
  MessagesSquare,
  Plus,
  RefreshCw,
  Send,
  Square,
  Trash2,
  User,
  X
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
  type DraftPlan,
  type DraftTask,
  type IssueProvider,
  type IssueSplit,
  previewIssuesFromChat,
  resolveChatModel,
  streamChat
} from '@/runtime/chat-client'
import {
  type Conversation,
  type FiledResult,
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

/** Human name for a tracker (spec 003 draft-review copy). */
const providerLabel = (p: IssueProvider): string => (p === 'linear' ? 'Linear' : 'GitHub')

/** Parse the comma/newline-separated label input into a clean list. */
const parseLabels = (raw: string): string[] =>
  raw
    .split(/[,\n]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0)

/** Issue number of a filed issue: the tracker id when numeric (GitHub), else
 *  the `/issues/<n>` tail of its URL. Null for Linear ids like `ENG-123`. */
const parseIssueNumber = (issue: { id?: string; url: string }): number | null => {
  if (issue.id && /^\d+$/.test(issue.id)) return Number(issue.id)
  const m = /\/issues\/(\d+)(?:[/?#]|$)/.exec(issue.url)
  return m ? Number(m[1]) : null
}

export default function ChatPage() {
  const repos = useAppStore((s) => s.repos)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const setActiveView = useAppStore((s) => s.setActiveView)
  const openTaskPage = useAppStore((s) => s.openTaskPage)

  const [conversations, setConversations] = useState<Conversation[]>(() => loadConversations())
  const [activeId, setActiveId] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Spec 003: the editable draft shown BEFORE any issue is filed. Non-null = the
  // review modal is open. `previewing` covers the extract/regenerate call;
  // `creating` (above) covers the confirm/file call. split/provider/labels are the
  // filing choices; `draftDirty` gates the regenerate-discards-edits confirm.
  const [draftPlan, setDraftPlan] = useState<DraftPlan | null>(null)
  const [previewing, setPreviewing] = useState(false)
  const [split, setSplit] = useState<IssueSplit>('single')
  const [provider, setProvider] = useState<IssueProvider>('github')
  const [labelsInput, setLabelsInput] = useState('')
  const [draftDirty, setDraftDirty] = useState(false)

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

  const appendAssistant = useCallback((id: string, content: string, filed?: FiledResult) => {
    setConversations((prev) =>
      prev.map((c) =>
        c.id === id
          ? {
              ...c,
              messages: [...c.messages, { role: 'assistant', content, ...(filed ? { filed } : {}) }],
              updatedAt: Date.now()
            }
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

  // Spec 003: extract the agreed breakdown into an editable DRAFT and open the
  // review modal — files NOTHING. Confirm (below) is the only mutation.
  const openPreview = useCallback(async () => {
    if (busy || previewing || creating || !active || messages.length === 0) return
    setPreviewing(true)
    setError(null)
    try {
      const plan = await previewIssuesFromChat(
        messages.map((m) => ({ role: m.role, content: m.content })),
        // The server infers the GitHub repo from the workspace's `origin`.
        { workdir: workspace?.path }
      )
      setDraftPlan(plan)
      setDraftDirty(false)
    } catch (e2) {
      setError(e2 instanceof Error ? e2.message : String(e2))
    } finally {
      setPreviewing(false)
    }
  }, [busy, previewing, creating, active, messages, workspace])

  // Re-extract, replacing the draft. Unsaved edits are discarded (confirmed
  // first) — the regenerated plan is authoritative.
  const regenerateDraft = useCallback(async () => {
    if (
      draftDirty &&
      !window.confirm('Regenerate will discard your edits and produce a fresh plan. Continue?')
    ) {
      return
    }
    await openPreview()
  }, [draftDirty, openPreview])

  // File the (edited) draft VERBATIM into the chosen tracker, then append a
  // summary turn linking the created issues and close the modal.
  const confirmDraft = useCallback(async () => {
    if (!active || !draftPlan || creating) return
    const convoId = active.id
    const plan = draftPlan
    setCreating(true)
    setError(null)
    try {
      const result = await createIssuesFromChat(
        messages.map((m) => ({ role: m.role, content: m.content })),
        { workdir: workspace?.path, provider, plan, split, labels: parseLabels(labelsInput) }
      )
      const where = result.repo
        ? `\`${result.repo}\``
        : workspace
          ? workspace.displayName
          : providerLabel(provider)
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
      // The markdown lines stay as the turn's `content` (old chats / exports);
      // `filed` is what the transcript actually renders — a card whose rows
      // click through to the board.
      appendAssistant(convoId, lines.join('\n'), {
        provider,
        repo: result.repo ?? null,
        issues: result.created,
        failed: result.failed
      })
      setDraftPlan(null) // close on success — no double-file
    } catch (e2) {
      setError(e2 instanceof Error ? e2.message : String(e2))
    } finally {
      setCreating(false)
    }
  }, [active, draftPlan, creating, messages, workspace, provider, split, labelsInput, appendAssistant])

  // Draft edit helpers — every edit marks the draft dirty (Regenerate then warns).
  const patchPlan = useCallback((patch: Partial<DraftPlan>) => {
    setDraftPlan((p) => (p ? { ...p, ...patch } : p))
    setDraftDirty(true)
  }, [])
  const patchTask = useCallback((i: number, patch: Partial<DraftTask>) => {
    setDraftPlan((p) => (p ? { ...p, tasks: p.tasks.map((t, j) => (j === i ? { ...t, ...patch } : t)) } : p))
    setDraftDirty(true)
  }, [])
  const addTask = useCallback(() => {
    setDraftPlan((p) =>
      p ? { ...p, tasks: [...p.tasks, { title: '', detail: '', priority: 'medium' }] } : p
    )
    setDraftDirty(true)
  }, [])
  const removeTask = useCallback((i: number) => {
    setDraftPlan((p) => (p ? { ...p, tasks: p.tasks.filter((_, j) => j !== i) } : p))
    setDraftDirty(true)
  }, [])

  const useExample = useCallback((text: string) => {
    setDraft(text)
    textareaRef.current?.focus()
  }, [])

  // Filed-card row click → the Tasks board. For a GitHub issue with a parsable
  // number we land with THAT issue's detail dialog already open (the same
  // `openGitHubWorkItem` hand-off the sidebar uses); otherwise (Linear, or no
  // number) we open the board on the right tracker tab and let the fresh list
  // show it.
  const openIssueOnBoard = useCallback(
    (filed: FiledResult, issue: FiledResult['issues'][number]) => {
      const number = parseIssueNumber(issue)
      if (filed.provider === 'github' && number != null) {
        openTaskPage({
          preselectedRepoId: workspaceId ?? undefined,
          taskSource: 'github',
          openGitHubWorkItem: {
            id: issue.url || String(number),
            type: 'issue',
            number,
            title: issue.title,
            state: 'open',
            url: issue.url,
            labels: [],
            updatedAt: new Date().toISOString(),
            author: null
          },
          openGitHubInitialTab: 'conversation'
        })
        return
      }
      openTaskPage({
        preselectedRepoId: workspaceId ?? undefined,
        taskSource: filed.provider === 'linear' ? 'linear' : 'github'
      })
    },
    [openTaskPage, workspaceId]
  )

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
                    onOpenIssue={openIssueOnBoard}
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
              <div className="mx-auto mb-2.5 flex max-w-[760px] items-center justify-between gap-3 rounded-lg border border-border/70 bg-card/60 py-1.5 pl-3 pr-1.5">
                <span className="min-w-0 truncate text-[11.5px] text-muted-foreground">
                  {workspace
                    ? `Ready to file? Review and edit the draft first — repo inferred from ${workspace.displayName}.`
                    : 'Pick a workspace above to file issues into.'}
                </span>
                <button
                  type="button"
                  onClick={() => void openPreview()}
                  disabled={busy || previewing || creating || messages.length === 0 || !workspace}
                  className="inline-flex flex-none items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[12.5px] font-medium hover:border-foreground/30 hover:bg-accent disabled:opacity-40"
                >
                  {previewing ? (
                    <Loader2 className="size-3.5 animate-spin" />
                  ) : (
                    <Eye className="size-3.5" />
                  )}
                  {previewing ? 'Preparing preview…' : 'Preview issues'}
                </button>
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

      {draftPlan ? (
        <DraftReview
          plan={draftPlan}
          split={split}
          provider={provider}
          labelsInput={labelsInput}
          previewing={previewing}
          creating={creating}
          dirty={draftDirty}
          workspaceName={workspace?.displayName ?? null}
          onPatchPlan={patchPlan}
          onPatchTask={patchTask}
          onAddTask={addTask}
          onRemoveTask={removeTask}
          onSplit={setSplit}
          onProvider={setProvider}
          onLabels={setLabelsInput}
          onRegenerate={() => void regenerateDraft()}
          onConfirm={() => void confirmDraft()}
          onCancel={() => setDraftPlan(null)}
        />
      ) : null}
    </div>
  )
}

/** Task-priority accents for the draft review — the rail encodes priority so
 *  each row reads at a glance without a dropdown. */
const PRIORITY_RAIL: Record<DraftTask['priority'], string> = {
  high: 'bg-red-400/80',
  medium: 'bg-amber-400/70',
  low: 'bg-border'
}
const PRIORITY_ACTIVE: Record<DraftTask['priority'], string> = {
  high: 'border-red-400/40 bg-red-400/10 text-red-400',
  medium: 'border-amber-400/40 bg-amber-400/10 text-amber-400',
  low: 'border-border bg-accent text-foreground'
}

/** Spec 003 — the review modal shown BEFORE any issue is filed. Reads as a
 *  document being prepared, not a form: the title/summary/tasks edit in place,
 *  each task carries a priority rail, the filing choices live in one quiet
 *  strip, and the confirm button states exactly what will happen. Escape
 *  cancels (unless a call is in flight); the title is focused on open. */
function DraftReview(props: {
  plan: DraftPlan
  split: IssueSplit
  provider: IssueProvider
  labelsInput: string
  previewing: boolean
  creating: boolean
  dirty: boolean
  workspaceName: string | null
  onPatchPlan: (patch: Partial<DraftPlan>) => void
  onPatchTask: (i: number, patch: Partial<DraftTask>) => void
  onAddTask: () => void
  onRemoveTask: (i: number) => void
  onSplit: (s: IssueSplit) => void
  onProvider: (p: IssueProvider) => void
  onLabels: (v: string) => void
  onRegenerate: () => void
  onConfirm: () => void
  onCancel: () => void
}) {
  const { plan, split, provider, previewing, creating, onCancel } = props
  const working = previewing || creating
  const issueCount = split === 'per_task' ? plan.tasks.length : 1
  const canConfirm = plan.title.trim().length > 0 && plan.tasks.length > 0 && !working

  const titleRef = useRef<HTMLInputElement | null>(null)
  useEffect(() => {
    titleRef.current?.focus()
  }, [])
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape' && !working) onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [working, onCancel])

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      role="dialog"
      aria-modal="true"
      aria-label="Review before filing"
    >
      <div className="flex max-h-[88vh] w-full max-w-[680px] flex-col overflow-hidden rounded-xl border border-border bg-card shadow-xl">
        {/* header — eyebrow + route: where this draft is going */}
        <div className="flex flex-none items-center gap-3 border-b border-border px-5 py-3">
          <span className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
            Review before filing
          </span>
          <span className="ml-auto min-w-0 truncate text-right font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
            {issueCount} issue{issueCount === 1 ? '' : 's'} → {providerLabel(provider)}
            {props.workspaceName ? ` · ${props.workspaceName}` : ''}
          </span>
          <button
            type="button"
            onClick={onCancel}
            disabled={working}
            className="-mr-1 flex-none rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40"
            aria-label="Cancel"
          >
            <X className="size-4" />
          </button>
        </div>

        {/* body — the issue as a document: title, summary, then the task list */}
        <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-4 pt-3.5">
          <input
            ref={titleRef}
            value={plan.title}
            onChange={(e) => props.onPatchPlan({ title: e.target.value })}
            placeholder="Feature title"
            aria-label="Feature title"
            className="w-full bg-transparent text-[16px] font-semibold tracking-tight text-foreground placeholder:text-muted-foreground/60 focus:outline-none"
          />
          <textarea
            value={plan.summary}
            onChange={(e) => props.onPatchPlan({ summary: e.target.value })}
            rows={2}
            placeholder="One-paragraph summary — becomes the issue body's opening."
            aria-label="Summary"
            className="mt-1.5 w-full resize-y bg-transparent text-[12.5px] leading-relaxed text-muted-foreground placeholder:text-muted-foreground/50 focus:text-foreground focus:outline-none"
          />

          <div className="mt-3 flex items-center justify-between border-t border-border/60 pt-3">
            <span className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
              Tasks · {plan.tasks.length}
            </span>
            <button
              type="button"
              onClick={props.onAddTask}
              className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <Plus className="size-3" /> Add task
            </button>
          </div>

          <ul className="mt-1 flex flex-col">
            {plan.tasks.map((t, i) => (
              <li
                key={i}
                className="group relative rounded-md py-2 pl-3.5 pr-1 transition-colors hover:bg-foreground/[0.03]"
              >
                <span
                  aria-hidden
                  className={cn(
                    'absolute inset-y-2 left-0 w-[2px] rounded-full transition-colors',
                    PRIORITY_RAIL[t.priority]
                  )}
                />
                <div className="flex items-center gap-2">
                  <input
                    value={t.title}
                    onChange={(e) => props.onPatchTask(i, { title: e.target.value })}
                    placeholder="Task title"
                    className="min-w-0 flex-1 bg-transparent text-[13px] font-medium text-foreground placeholder:text-muted-foreground/60 focus:outline-none"
                  />
                  <div
                    className="inline-flex flex-none rounded-md border border-transparent"
                    role="radiogroup"
                    aria-label="Priority"
                  >
                    {(['high', 'medium', 'low'] as const).map((p) => (
                      <button
                        key={p}
                        type="button"
                        role="radio"
                        aria-checked={t.priority === p}
                        title={`${p[0].toUpperCase()}${p.slice(1)} priority`}
                        onClick={() => props.onPatchTask(i, { priority: p })}
                        className={cn(
                          'rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase',
                          t.priority === p
                            ? PRIORITY_ACTIVE[p]
                            : 'border-transparent text-muted-foreground/50 hover:text-muted-foreground'
                        )}
                      >
                        {p[0]}
                      </button>
                    ))}
                  </div>
                  <button
                    type="button"
                    onClick={() => props.onRemoveTask(i)}
                    className="flex-none rounded-md p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-red-400 focus-visible:opacity-100 group-hover:opacity-100"
                    aria-label={`Remove task: ${t.title || 'untitled'}`}
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                </div>
                <input
                  value={t.detail}
                  onChange={(e) => props.onPatchTask(i, { detail: e.target.value })}
                  placeholder="Add detail…"
                  className="mt-0.5 w-full bg-transparent text-[12px] text-muted-foreground placeholder:text-muted-foreground/40 focus:text-foreground focus:outline-none"
                />
              </li>
            ))}
            {plan.tasks.length === 0 ? (
              <li className="py-2 text-[12px] text-muted-foreground">
                No tasks — add one, or regenerate the draft.
              </li>
            ) : null}
          </ul>
        </div>

        {/* filing strip — the choices, one quiet row */}
        <div className="flex flex-none flex-wrap items-center gap-x-3 gap-y-2 border-t border-border bg-background/60 px-5 py-2.5">
          <SegButtons
            value={split}
            onChange={(v) => props.onSplit(v as IssueSplit)}
            options={[
              { v: 'single', label: 'One issue + checklist' },
              { v: 'per_task', label: 'Per task' }
            ]}
          />
          <SegButtons
            value={provider}
            onChange={(v) => props.onProvider(v as IssueProvider)}
            options={[
              { v: 'github', label: 'GitHub' },
              { v: 'linear', label: 'Linear' }
            ]}
          />
          <input
            value={props.labelsInput}
            onChange={(e) => props.onLabels(e.target.value)}
            placeholder={provider === 'linear' ? 'labels — GitHub only for now' : 'labels: enhancement, area/chat'}
            aria-label="Labels, comma-separated"
            disabled={provider === 'linear'}
            className="min-w-[9rem] flex-1 bg-transparent font-mono text-[11.5px] text-foreground placeholder:text-muted-foreground/50 focus:outline-none disabled:opacity-50"
          />
        </div>

        {/* footer — the consequence, stated exactly */}
        <div className="flex flex-none items-center justify-between gap-2 border-t border-border px-5 py-3">
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={props.onRegenerate}
              disabled={working}
              className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1 text-[12.5px] hover:bg-accent disabled:opacity-40"
            >
              {previewing ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <RefreshCw className="size-3.5" />
              )}
              Regenerate
            </button>
            {props.dirty ? (
              <span className="font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
                edited
              </span>
            ) : null}
          </div>
          <div className="flex min-w-0 items-center gap-2.5">
            <span className="hidden truncate text-[11px] text-muted-foreground sm:inline">
              Nothing is created until you confirm
            </span>
            <button
              type="button"
              onClick={onCancel}
              disabled={working}
              className="flex-none rounded-md border border-border px-2.5 py-1 text-[12.5px] hover:bg-accent disabled:opacity-40"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={props.onConfirm}
              disabled={!canConfirm}
              className="inline-flex flex-none items-center gap-1.5 rounded-md bg-primary px-3 py-1 text-[12.5px] font-medium text-primary-foreground hover:opacity-85 disabled:opacity-40"
            >
              {creating ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : provider === 'linear' ? (
                <Check className="size-3.5" />
              ) : (
                <Github className="size-3.5" />
              )}
              {creating ? 'Filing…' : `Create ${issueCount} issue${issueCount === 1 ? '' : 's'}`}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

/** A small segmented toggle used for the split / provider choices. */
function SegButtons(props: {
  value: string
  onChange: (v: string) => void
  options: { v: string; label: string }[]
}) {
  return (
    <div className="inline-flex rounded-md border border-border p-0.5">
      {props.options.map((o) => (
        <button
          key={o.v}
          type="button"
          onClick={() => props.onChange(o.v)}
          className={cn(
            'rounded px-2 py-1 text-[12px]',
            props.value === o.v
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:bg-accent'
          )}
        >
          {o.label}
        </button>
      ))}
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
 *  markdown — or, for a filing-summary turn, the clickable issues card — with
 *  the optional reasoning trace above and a live indicator while streaming. */
function Bubble({
  turn,
  streaming,
  onOpenIssue
}: {
  turn: StoredTurn
  streaming: boolean
  onOpenIssue: (filed: FiledResult, issue: FiledResult['issues'][number]) => void
}) {
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
            {turn.filed ? (
              <FiledCard filed={turn.filed} onOpenIssue={onOpenIssue} />
            ) : turn.content ? (
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

/** The filing-summary card: what was created, where — and the bridge onward.
 *  Clicking a row opens the Tasks board with that issue's detail; the trailing
 *  arrow opens the tracker in the browser. Failures are listed with their
 *  reasons, never hidden behind a count. */
function FiledCard({
  filed,
  onOpenIssue
}: {
  filed: FiledResult
  onOpenIssue: (filed: FiledResult, issue: FiledResult['issues'][number]) => void
}) {
  const n = filed.issues.length
  const destination = filed.repo ?? providerLabel(filed.provider)
  return (
    <div className="overflow-hidden rounded-2xl rounded-tl-sm border border-border bg-card">
      <div className="flex items-center gap-2 border-b border-border px-4 py-2.5">
        {filed.provider === 'linear' ? (
          <Check className="size-3.5 text-primary" />
        ) : (
          <Github className="size-3.5 text-muted-foreground" />
        )}
        <span className="min-w-0 truncate text-[12.5px] font-medium">
          {n > 0 ? `Filed to ${destination}` : `Nothing filed to ${destination}`}
        </span>
        <span className="ml-auto flex-none font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
          {n} issue{n === 1 ? '' : 's'}
        </span>
      </div>
      {n > 0 ? (
        <ul className="divide-y divide-border/60">
          {filed.issues.map((issue, i) => {
            const number = parseIssueNumber(issue)
            const ref = number != null ? `#${number}` : (issue.id ?? '—')
            return (
              <li key={i} className="flex items-stretch">
                <button
                  type="button"
                  onClick={() => onOpenIssue(filed, issue)}
                  title="Open on the board"
                  className="group flex min-w-0 flex-1 items-center gap-2.5 px-4 py-2.5 text-left transition-colors hover:bg-accent/60"
                >
                  <span className="flex-none font-mono text-[12px] text-muted-foreground">
                    {ref}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-[13px]">{issue.title}</span>
                  <span className="inline-flex flex-none items-center gap-1 font-mono text-[10px] uppercase tracking-wide text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100">
                    <Columns3 className="size-3.5" /> board
                  </span>
                </button>
                {issue.url ? (
                  <a
                    href={issue.url}
                    target="_blank"
                    rel="noreferrer"
                    title={`Open on ${providerLabel(filed.provider)}`}
                    className="flex flex-none items-center border-l border-border/60 px-3 text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground"
                  >
                    <ArrowUpRight className="size-3.5" />
                  </a>
                ) : null}
              </li>
            )
          })}
        </ul>
      ) : null}
      {filed.failed.length > 0 ? (
        <div className="border-t border-border/60 px-4 py-2.5">
          <div className="mb-1 font-mono text-[10px] uppercase tracking-wide text-red-400">
            {filed.failed.length} not created
          </div>
          <ul className="flex flex-col gap-0.5">
            {filed.failed.map((f, i) => (
              <li key={i} className="text-[12px] text-muted-foreground">
                <span className="text-foreground/80">{f.title}</span> — {f.error}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
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
