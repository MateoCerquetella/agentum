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

export default function ChatPage({ pinnedRepo }: { pinnedRepo?: Repo | null } = {}) {
  const repos = useAppStore((s) => s.repos)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const setActiveView = useAppStore((s) => s.setActiveView)

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
  // When embedded in the Project Hub (`pinnedRepo`), the hub's project IS the
  // workspace — no picker, no persistence, and the stored global choice is
  // left untouched.
  const [workspaceId, setWorkspaceId] = useState<string | null>(() => {
    if (pinnedRepo) return pinnedRepo.id
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
    if (pinnedRepo) {
      setWorkspaceId(pinnedRepo.id)
      return
    }
    if (repos.length === 0) return
    setWorkspaceId((cur) => {
      if (cur && repos.some((r) => r.id === cur)) return cur
      return activeRepoId ?? repos[0]?.id ?? null
    })
  }, [repos, activeRepoId, pinnedRepo])
  useEffect(() => {
    if (pinnedRepo) return
    try {
      if (workspaceId) localStorage.setItem(WORKSPACE_KEY, workspaceId)
    } catch {
      /* storage may be unavailable — selection still works this session */
    }
  }, [workspaceId, pinnedRepo])
  const workspace = useMemo(
    () => (pinnedRepo ? pinnedRepo : (repos.find((r) => r.id === workspaceId) ?? null)),
    [repos, workspaceId, pinnedRepo]
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

  // The history rail's list. In the Project Hub only threads grounded in the
  // pinned project appear; the global Chat view keeps showing everything
  // (including pre-hub conversations that carry no repoId).
  const visibleConversations = useMemo(
    () => (pinnedRepo ? conversations.filter((c) => c.repoId === pinnedRepo.id) : conversations),
    [conversations, pinnedRepo]
  )

  const active = useMemo(
    () => conversations.find((c) => c.id === activeId) ?? null,
    [conversations, activeId]
  )
  const messages = active?.messages ?? []
  const hasAssistantReply = messages.some((m) => m.role === 'assistant' && m.content.trim().length > 0)

  // Keep the open transcript inside the pinned project's scope: switching hub
  // projects (or opening the hub while a foreign thread was active) resets to
  // the empty state rather than showing another project's conversation.
  useEffect(() => {
    if (!pinnedRepo || activeId == null) return
    const activeInScope = visibleConversations.some((c) => c.id === activeId)
    if (!activeInScope) setActiveId(null)
  }, [pinnedRepo, activeId, visibleConversations])

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
            updatedAt: now,
            // Scope the thread to the project that grounded it, so the Project
            // Hub's per-project history can filter without a migration.
            repoId: workspace?.id
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
      appendAssistant(convoId, lines.join('\n'))
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

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      {/* The Project Hub wraps this page in its own header + tab strip, so the
          full-page chrome only renders standalone. */}
      {pinnedRepo ? null : (
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
      )}

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
            {visibleConversations.length === 0 ? (
              <div className="px-3 py-2 text-[12px] text-muted-foreground">No chats yet.</div>
            ) : (
              visibleConversations.map((c) => {
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
          {/* workspace + model + thinking toolbar. Pinned (hub) mode drops the
              workspace picker — the hub's project IS the workspace. */}
          <div className="flex h-11 flex-none items-center gap-2 border-b border-border px-4">
            {pinnedRepo ? null : (
              <WorkspacePicker
                repos={repos}
                workspaceId={workspaceId}
                onChange={setWorkspaceId}
                disabled={busy}
              />
            )}
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
                    onClick={() => void openPreview()}
                    disabled={busy || previewing || creating || messages.length === 0 || !workspace}
                    className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[12.5px] font-medium hover:border-foreground/30 hover:bg-accent disabled:opacity-40"
                  >
                    {previewing ? (
                      <Loader2 className="size-3.5 animate-spin" />
                    ) : (
                      <Eye className="size-3.5" />
                    )}
                    {previewing ? 'Preparing preview…' : 'Preview issues'}
                  </button>
                </div>
                <div className="text-right text-[11px] text-muted-foreground">
                  {workspace
                    ? `Review & edit before filing — GitHub repo inferred from ${workspace.displayName}.`
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

      {draftPlan ? (
        <DraftReview
          plan={draftPlan}
          split={split}
          provider={provider}
          labelsInput={labelsInput}
          previewing={previewing}
          creating={creating}
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

/** Spec 003 — the review modal shown BEFORE any issue is filed. Fully editable
 *  (title / summary / tasks + priority), plus split / provider / labels choices,
 *  with Regenerate (re-extract) and Confirm (file the shown draft verbatim). */
function DraftReview(props: {
  plan: DraftPlan
  split: IssueSplit
  provider: IssueProvider
  labelsInput: string
  previewing: boolean
  creating: boolean
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
  const { plan, split, provider, previewing, creating } = props
  const working = previewing || creating
  const issueCount = split === 'per_task' ? plan.tasks.length : 1
  const canConfirm = plan.title.trim().length > 0 && plan.tasks.length > 0 && !working
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      role="dialog"
      aria-modal="true"
    >
      <div className="flex max-h-[88vh] w-full max-w-[720px] flex-col overflow-hidden rounded-xl border border-border bg-card shadow-xl">
        {/* header */}
        <div className="flex flex-none items-center justify-between border-b border-border px-4 py-3">
          <div>
            <div className="text-[13px] font-semibold">Review before filing</div>
            <div className="text-[11px] text-muted-foreground">
              Nothing is created until you confirm · {issueCount} issue
              {issueCount === 1 ? '' : 's'} → {providerLabel(provider)}
              {props.workspaceName ? ` · ${props.workspaceName}` : ''}
            </div>
          </div>
          <button
            type="button"
            onClick={props.onCancel}
            disabled={working}
            className="rounded-md p-1 text-muted-foreground hover:bg-accent disabled:opacity-40"
            aria-label="Cancel"
          >
            <X className="size-4" />
          </button>
        </div>

        {/* body */}
        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
          <label className="mb-1 block text-[11px] font-medium text-muted-foreground">
            Feature title
          </label>
          <input
            value={plan.title}
            onChange={(e) => props.onPatchPlan({ title: e.target.value })}
            className="mb-3 w-full rounded-md border border-border bg-background px-2.5 py-1.5 text-[13px] focus:border-foreground/30 focus:outline-none"
          />

          <label className="mb-1 block text-[11px] font-medium text-muted-foreground">Summary</label>
          <textarea
            value={plan.summary}
            onChange={(e) => props.onPatchPlan({ summary: e.target.value })}
            rows={2}
            className="mb-3 w-full resize-y rounded-md border border-border bg-background px-2.5 py-1.5 text-[13px] focus:border-foreground/30 focus:outline-none"
          />

          <div className="mb-1 flex items-center justify-between">
            <span className="text-[11px] font-medium text-muted-foreground">
              Tasks ({plan.tasks.length})
            </span>
            <button
              type="button"
              onClick={props.onAddTask}
              className="inline-flex items-center gap-1 rounded-md border border-border px-1.5 py-0.5 text-[11px] hover:bg-accent"
            >
              <Plus className="size-3" /> Add task
            </button>
          </div>
          <div className="flex flex-col gap-2">
            {plan.tasks.map((t, i) => (
              <div key={i} className="rounded-md border border-border bg-background p-2">
                <div className="flex items-center gap-2">
                  <input
                    value={t.title}
                    onChange={(e) => props.onPatchTask(i, { title: e.target.value })}
                    placeholder="Task title"
                    className="min-w-0 flex-1 rounded-md border border-border bg-card px-2 py-1 text-[12.5px] focus:border-foreground/30 focus:outline-none"
                  />
                  <select
                    value={t.priority}
                    onChange={(e) =>
                      props.onPatchTask(i, { priority: e.target.value as DraftTask['priority'] })
                    }
                    className="flex-none rounded-md border border-border bg-card px-1.5 py-1 text-[12px]"
                  >
                    <option value="high">High</option>
                    <option value="medium">Medium</option>
                    <option value="low">Low</option>
                  </select>
                  <button
                    type="button"
                    onClick={() => props.onRemoveTask(i)}
                    className="flex-none rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-red-400"
                    aria-label="Remove task"
                  >
                    <Trash2 className="size-3.5" />
                  </button>
                </div>
                <textarea
                  value={t.detail}
                  onChange={(e) => props.onPatchTask(i, { detail: e.target.value })}
                  rows={1}
                  placeholder="Detail (optional)"
                  className="mt-1.5 w-full resize-y rounded-md border border-border bg-card px-2 py-1 text-[12px] focus:border-foreground/30 focus:outline-none"
                />
              </div>
            ))}
            {plan.tasks.length === 0 ? (
              <div className="text-[12px] text-muted-foreground">
                No tasks — add one, or regenerate.
              </div>
            ) : null}
          </div>

          <div className="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div>
              <div className="mb-1 text-[11px] font-medium text-muted-foreground">How to file</div>
              <SegButtons
                value={split}
                onChange={(v) => props.onSplit(v as IssueSplit)}
                options={[
                  { v: 'single', label: 'One issue + checklist' },
                  { v: 'per_task', label: 'One issue per task' }
                ]}
              />
            </div>
            <div>
              <div className="mb-1 text-[11px] font-medium text-muted-foreground">Tracker</div>
              <SegButtons
                value={provider}
                onChange={(v) => props.onProvider(v as IssueProvider)}
                options={[
                  { v: 'github', label: 'GitHub' },
                  { v: 'linear', label: 'Linear' }
                ]}
              />
            </div>
          </div>

          <div className="mt-3">
            <label className="mb-1 block text-[11px] font-medium text-muted-foreground">
              Labels (comma-separated)
            </label>
            <input
              value={props.labelsInput}
              onChange={(e) => props.onLabels(e.target.value)}
              placeholder="enhancement, area/chat"
              className="w-full rounded-md border border-border bg-background px-2.5 py-1.5 text-[12.5px] focus:border-foreground/30 focus:outline-none"
            />
            {provider === 'linear' ? (
              <div className="mt-1 text-[11px] text-muted-foreground">
                Labels are applied to GitHub only for now.
              </div>
            ) : null}
          </div>
        </div>

        {/* footer */}
        <div className="flex flex-none items-center justify-between gap-2 border-t border-border px-4 py-3">
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
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={props.onCancel}
              disabled={working}
              className="rounded-md border border-border px-2.5 py-1 text-[12.5px] hover:bg-accent disabled:opacity-40"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={props.onConfirm}
              disabled={!canConfirm}
              className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1 text-[12.5px] font-medium text-primary-foreground hover:opacity-85 disabled:opacity-40"
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
