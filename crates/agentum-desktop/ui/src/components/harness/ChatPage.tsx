// Chat — a real conversational front door to the Spec→tickets pipeline (#48).
// Describe a feature in plain words and the server-side Socratic interviewer
// (`POST /api/chat/stream`) asks a few clarifying questions, then proposes a task
// breakdown the user can file as GitHub issues. The target repo is INFERRED from
// the selected workspace (a picker in the toolbar) — that same workspace grounds
// the interview — so there's no manual owner/repo entry. The reply streams in
// token-by-token; an optional extended-thinking trace is shown above each answer.
//
// Conversations and in-flight streams live in the module-level `chat-store` —
// NOT in this component — so a reply keeps streaming when the page unmounts
// (view switch, hub tab change) and is intact when the user comes back. This
// page is a subscriber; the only mutation it owns is the issue preview/file
// flow ("Preview issues" → review modal → confirm).
import { type FormEvent, type KeyboardEvent, memo, type MouseEvent as ReactMouseEvent, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import {
  ArrowUp,
  ArrowUpRight,
  Brain,
  Check,
  ChevronDown,
  Columns3,
  Eye,
  FolderGit2,
  Github,
  Loader2,
  MessagesSquare,
  Plus,
  RefreshCw,
  Square,
  Trash2,
  X,
  Zap
} from 'lucide-react'

import { useAppStore } from '@/store'
import { openBoardSurface } from '@/lib/board-route'
import { cn } from '@/lib/utils'
import { api } from '@/tauri'
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
  resolveChatModel
} from '@/runtime/chat-client'
import { contextWarningText } from '@/lib/chat-context-status'
import { clampStage, type IntakeMode, normalizeIntake } from '@/lib/socratic-intake'
import { type Conversation, type FiledResult, type StoredTurn } from '@/runtime/chat-history'
import {
  appendAssistantTurn,
  deleteConversation,
  dismissStreamError,
  getChatSnapshot,
  sendChatMessage,
  stopStream,
  subscribeChat
} from '@/runtime/chat-store'

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

/** Stable empty transcript so effects keyed on `messages` don't re-fire when no
 *  conversation is selected. */
const NO_MESSAGES: StoredTurn[] = []

export default function ChatPage({ pinnedRepo }: { pinnedRepo?: Repo | null } = {}) {
  const repos = useAppStore((s) => s.repos)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const openTaskPage = useAppStore((s) => s.openTaskPage)
  const openProjectHub = useAppStore((s) => s.openProjectHub)

  // Conversations + stream state come from the module store, so a reply that is
  // mid-stream when this page unmounts is still streaming when it remounts.
  const chat = useSyncExternalStore(subscribeChat, getChatSnapshot)
  const conversations = chat.conversations

  const [activeId, setActiveId] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  // #258: the intake mode for a NEW chat. Null = not chosen yet, and the
  // composer stays locked until one of the two large cards is picked.
  const [pendingMode, setPendingMode] = useState<IntakeMode | null>(null)
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

  const textareaRef = useRef<HTMLTextAreaElement | null>(null)

  // Auto-grow the composer with its content. The cap matches the textarea's
  // max-h-40 (160px); past it the textarea scrolls internally.
  useEffect(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`
  }, [draft])

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
  const messages = active?.messages ?? NO_MESSAGES
  const hasAssistantReply = messages.some((m) => m.role === 'assistant' && m.content.trim().length > 0)

  // Spec 008 F2: the open thread's intake (Fast/Complex + socratic pass), so the
  // composer can show a Complex progress indicator; null on the empty state.
  const activeIntake = useMemo(() => (active ? normalizeIntake(active.intake) : null), [active])

  // Busy = THIS conversation is streaming. Other conversations may stream in the
  // background at the same time (the sidebar shows a spinner on each).
  const busy = activeId != null && !!chat.streaming[activeId]
  const streamError = activeId != null ? (chat.errors[activeId] ?? null) : null
  // Spec 009 (#361): the server said this workspace-backed chat couldn't be
  // grounded — warn visibly instead of leaving the model to apologize for it.
  const contextMissing = activeId != null && !!chat.contextMissing[activeId]

  // Keep the open transcript inside the pinned project's scope: switching hub
  // projects (or opening the hub while a foreign thread was active) resets to
  // the empty state rather than showing another project's conversation.
  useEffect(() => {
    if (!pinnedRepo || activeId == null) return
    const activeInScope = visibleConversations.some((c) => c.id === activeId)
    if (!activeInScope) setActiveId(null)
  }, [pinnedRepo, activeId, visibleConversations])

  // Auto-follow the newest content of the OPEN conversation only — background
  // streams updating other threads must not yank the scroll. Instant while
  // streaming (token follow), smooth otherwise.
  const bottomRef = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: busy ? 'auto' : 'smooth', block: 'end' })
  }, [messages, busy])

  const startNewChat = useCallback(() => {
    // Deliberately does NOT stop an in-flight stream — it keeps going in the
    // background and the sidebar shows its progress.
    setActiveId(null)
    setDraft('')
    setPendingMode(null)
    setError(null)
    // Default the picker back to the stored preference for a fresh chat.
    setModel(readStoredModel())
    setThinking(readStoredThinking())
    textareaRef.current?.focus()
  }, [])

  const selectConversation = useCallback((c: Conversation) => {
    // Switching away from a streaming thread is fine — the store keeps it going.
    setActiveId(c.id)
    setError(null)
    setModel(c.model || DEFAULT_CHAT_MODEL)
    setThinking(!!c.thinking)
  }, [])

  const removeConversation = useCallback(
    (c: Conversation) => {
      const ok = window.confirm(`Delete "${c.title}"? This can't be undone.`)
      if (!ok) return
      deleteConversation(c.id)
      setActiveId((cur) => (cur === c.id ? null : cur))
    },
    []
  )

  // Mirrors `activeId` synchronously. A second Enter before React re-renders
  // would read a stale `activeId` of null and mint a SECOND conversation for
  // the same prompt; going through the ref makes the store's already-streaming
  // guard catch the duplicate instead.
  const activeIdRef = useRef<string | null>(null)
  useEffect(() => {
    activeIdRef.current = activeId
  }, [activeId])

  // Spec 008 F2: send the draft in a chosen intake mode. For a NEW thread `mode`
  // picks Fast vs the staged Socratic interview; a CONTINUING thread inherits its
  // stored mode/stage inside the store (which also advances the socratic pass).
  const submitWith = useCallback(
    (mode: IntakeMode) => {
      const text = draft.trim()
      if (!text || busy) return
      setError(null)
      const id = sendChatMessage({
        conversationId: activeIdRef.current,
        text,
        model,
        thinking,
        workdir: workspace?.path,
        // Scope new threads to the project that grounded them, so the Project
        // Hub's per-project history can filter without a migration.
        repoId: workspace?.id,
        mode
      })
      activeIdRef.current = id
      setActiveId(id)
      setDraft('')
    },
    [draft, busy, model, thinking, workspace]
  )

  // #258: a NEW chat requires a deliberately chosen mode — Enter no longer
  // silently defaults to Fast (the old D4 "Enter stays Fast" invariant is
  // overridden). A continuing thread keeps its stored mode inside the store,
  // so the mode passed here is only read for the first message of a thread.
  const submit = useCallback(
    (e?: FormEvent) => {
      e?.preventDefault()
      if (activeIdRef.current == null && pendingMode == null) return
      submitWith(pendingMode ?? 'fast')
    },
    [submitWith, pendingMode]
  )

  const stop = useCallback(() => {
    if (activeId != null) stopStream(activeId)
  }, [activeId])

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
      // click through to the board. Appended via the store so it lands even if
      // the user navigated away while the filing ran.
      appendAssistantTurn(convoId, lines.join('\n'), {
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
  }, [active, draftPlan, creating, messages, workspace, provider, split, labelsInput])

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

  // Markdown links in assistant replies render as `target="_blank"` anchors,
  // which are dead inside the Tauri webview (no window.open handler) — the
  // same bug the FiledCard arrow had. Route every http(s) anchor in the
  // transcript through the native opener instead.
  const onTranscriptClick = useCallback((e: ReactMouseEvent<HTMLDivElement>) => {
    const anchor = (e.target as HTMLElement).closest?.('a[href]')
    const href = anchor?.getAttribute('href') ?? ''
    if (!/^https?:\/\//i.test(href)) return
    e.preventDefault()
    void api.shell.openUrl(href)
  }, [])

  // Filed-card row click → the Tasks board. For a GitHub issue with a parsable
  // number we land with THAT issue's detail dialog already open (the same
  // `openGitHubWorkItem` hand-off the sidebar uses); otherwise (Linear, or no
  // number) we open the board on the right tracker tab and let the fresh list
  // show it.
  const openIssueOnBoard = useCallback(
    (filed: FiledResult, issue: FiledResult['issues'][number]) => {
      const number = parseIssueNumber(issue)
      // Spec 007 (bugs 1+2): the effective repo — pinned (Project Hub) wins
      // over the picked workspace, mirroring the `workspace` memo above.
      const filedRepoId = pinnedRepo?.id ?? workspaceId ?? undefined
      if (filed.provider === 'github' && number != null) {
        openTaskPage({
          preselectedRepoId: filedRepoId,
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
            author: null,
            // Without a repoId the detail page can't resolve a repoPath, so
            // the details fetch never fires (body + author stay blank) and
            // "Start workspace from issue" opens the composer with no
            // initialRepoId. The Chat workspace IS the repo.
            repoId: filedRepoId ?? ''
          },
          openGitHubInitialTab: 'conversation'
        })
        return
      }
      // Spec 016 D2: a bare board open (no detail payload) routes to the hub's
      // Tasks tab (Projects page when no repo resolves). The taskSource seed
      // keeps a Linear filed-card landing on the Linear tab.
      openBoardSurface({
        preferredRepoId: filedRepoId,
        taskSource: filed.provider === 'linear' ? 'linear' : 'github'
      })
    },
    [openTaskPage, pinnedRepo, workspaceId]
  )

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      {/* The Project Hub wraps this page in its own header + tab strip, so the
          full-page chrome only renders standalone. */}
      {pinnedRepo ? null : (
        <DrillInHeader
          icon={MessagesSquare}
          title="Chat"
          actions={
            <button
              type="button"
              // Standalone only (pinnedRepo is null in this branch): route the
              // bare open through the D2 resolver — hub when a repo resolves.
              onClick={() =>
                openBoardSurface({ preferredRepoId: workspaceId ?? undefined, taskSource: 'github' })
              }
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
              className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-[13px] font-medium text-foreground/90 transition-colors hover:bg-accent"
            >
              <Plus className="size-3.5" /> New chat
            </button>
          </div>
          <div className="px-3.5 pb-1.5 text-[11px] font-medium text-muted-foreground">Chats</div>
          <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-3">
            {visibleConversations.length === 0 ? (
              <div className="px-3 py-2 text-[12px] text-muted-foreground">No chats yet.</div>
            ) : (
              visibleConversations.map((c) => {
                const isActive = c.id === activeId
                const isStreaming = !!chat.streaming[c.id]
                // Global view: a thread grounded in a project links to that
                // project's hub chat (ADE prototype "open a thread to scope
                // it"). Pinned (hub) mode skips the chip — every row is
                // already this project.
                const threadRepo =
                  !pinnedRepo && c.repoId ? (repos.find((r) => r.id === c.repoId) ?? null) : null
                return (
                  <div key={c.id} className="group relative">
                    {/* div-with-button-role (not <button>) so the project chip
                        below can be a real nested button without invalid
                        interactive nesting — same pattern as sidebar headers. */}
                    <div
                      role="button"
                      tabIndex={0}
                      onClick={() => selectConversation(c)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault()
                          selectConversation(c)
                        }
                      }}
                      className={cn(
                        'flex w-full cursor-pointer flex-col gap-0.5 rounded-md px-2.5 py-2 pr-8 text-left transition-colors',
                        isActive ? 'bg-accent' : 'hover:bg-foreground/5'
                      )}
                    >
                      <span className="truncate text-[13px]">{c.title}</span>
                      <span className="flex items-center gap-1.5 truncate text-[11px] text-muted-foreground">
                        {isStreaming ? (
                          <Loader2 className="size-3 flex-none animate-spin text-primary" aria-label="replying" />
                        ) : null}
                        {threadRepo ? (
                          <>
                            <button
                              type="button"
                              title={`Open ${threadRepo.displayName} hub`}
                              onClick={(e) => {
                                e.stopPropagation()
                                openProjectHub(threadRepo.id, 'chat')
                              }}
                              className="max-w-[9rem] truncate rounded-sm border border-border/70 px-1 py-px text-[10px] leading-none hover:border-foreground/40 hover:text-foreground"
                            >
                              {threadRepo.displayName}
                            </button>
                            <span aria-hidden>·</span>
                          </>
                        ) : null}
                        <span>{timeAgo(c.updatedAt)}</span>
                        <span aria-hidden>·</span>
                        <span className="truncate">{shortModel(c.model)}</span>
                        {c.thinking ? (
                          <Brain className="size-3 text-primary/70" aria-label="thinking" />
                        ) : null}
                      </span>
                    </div>
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
          {/* workspace + model + thinking — quiet ghost controls, ChatGPT-header
              style. Pinned (hub) mode drops the workspace picker — the hub's
              project IS the workspace. */}
          <div className="flex h-12 flex-none items-center gap-1 px-3">
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
          </div>

          {/* transcript */}
          <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4" onClick={onTranscriptClick}>
            <div className="mx-auto flex w-full max-w-[46rem] flex-col gap-6">
              {messages.length === 0 && !busy ? (
                <EmptyState onPick={useExample} />
              ) : (
                messages.map((m, i) => (
                  <Turn
                    key={i}
                    turn={m}
                    streaming={busy && i === messages.length - 1 && m.role === 'assistant'}
                    onOpenIssue={openIssueOnBoard}
                  />
                ))
              )}

              {contextMissing ? (
                <WarningBanner text={contextWarningText(workspace?.displayName ?? null)} />
              ) : null}
              {error ? <ErrorBanner text={error} onDismiss={() => setError(null)} /> : null}
              {streamError ? (
                <ErrorBanner
                  text={streamError}
                  onDismiss={() => activeId != null && dismissStreamError(activeId)}
                />
              ) : null}

              <div ref={bottomRef} />
            </div>
          </div>

          {/* composer */}
          <form onSubmit={submit} className="flex-none px-4 pb-3 pt-1">
            <div className="mx-auto w-full max-w-[46rem]">
              {hasAssistantReply ? (
                <div className="mb-2 flex items-center justify-between gap-3 rounded-2xl border border-border/60 bg-card/50 py-1.5 pl-3.5 pr-1.5">
                  <span className="min-w-0 truncate text-[12px] text-muted-foreground">
                    {workspace
                      ? `Review the drafted issues before anything is filed — repo inferred from ${workspace.displayName}.`
                      : 'Pick a workspace above to file issues into.'}
                  </span>
                  <button
                    type="button"
                    onClick={() => void openPreview()}
                    disabled={busy || previewing || creating || messages.length === 0 || !workspace}
                    className="inline-flex flex-none items-center gap-1.5 rounded-full border border-border bg-card px-3 py-1 text-[12.5px] font-medium transition-colors hover:bg-accent disabled:opacity-40"
                  >
                    {previewing ? (
                      <Loader2 className="size-3.5 animate-spin" />
                    ) : (
                      <Eye className="size-3.5" />
                    )}
                    {/* The interviewer system prompt (routes/chat.rs) names this
                        button — keep the label and that prompt in sync. */}
                    {previewing ? 'Preparing preview…' : 'Preview issues'}
                  </button>
                </div>
              ) : null}
              {/* #258 (supersedes spec 008 F2 pills): on a NEW chat the mode is a
                  deliberate pre-work choice — two large cards, picked BEFORE the
                  composer accepts input. No Enter=Fast default; per-feature, no
                  sticky preference. A Complex thread then shows its pass progress. */}
              {!active ? (
                <div className="mb-3 grid grid-cols-1 gap-2.5 sm:grid-cols-2">
                  <button
                    type="button"
                    onClick={() => {
                      setPendingMode('fast')
                      textareaRef.current?.focus()
                    }}
                    aria-pressed={pendingMode === 'fast'}
                    className={`flex flex-col items-start gap-1.5 rounded-2xl border p-4 text-left transition-colors ${
                      pendingMode === 'fast'
                        ? 'border-foreground/40 bg-accent ring-2 ring-foreground/15'
                        : 'border-border bg-card hover:border-foreground/25 hover:bg-accent/60'
                    }`}
                  >
                    <span className="inline-flex items-center gap-2 text-[14.5px] font-semibold">
                      <Zap className="size-4" />
                      Fast feature
                    </span>
                    <span className="text-[12.5px] leading-relaxed text-muted-foreground">
                      One prompt, straight to a reviewable draft. Best for small, clear asks.
                    </span>
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setPendingMode('socratic')
                      textareaRef.current?.focus()
                    }}
                    aria-pressed={pendingMode === 'socratic'}
                    className={`flex flex-col items-start gap-1.5 rounded-2xl border p-4 text-left transition-colors ${
                      pendingMode === 'socratic'
                        ? 'border-foreground/40 bg-accent ring-2 ring-foreground/15'
                        : 'border-border bg-card hover:border-foreground/25 hover:bg-accent/60'
                    }`}
                  >
                    <span className="inline-flex items-center gap-2 text-[14.5px] font-semibold">
                      <Brain className="size-4" />
                      Complex feature
                    </span>
                    <span className="text-[12.5px] leading-relaxed text-muted-foreground">
                      A guided interview that pins down scope first. Best for big or fuzzy work.
                    </span>
                  </button>
                </div>
              ) : activeIntake?.mode === 'socratic' ? (
                <div className="mb-2 flex items-center gap-1.5 text-[11px] text-muted-foreground/70">
                  <Brain className="size-3" />
                  {activeIntake.converged
                    ? 'Complex feature · spec defined — review with "Preview issues"'
                    : `Complex feature · pass ${clampStage(activeIntake.stage)} of 5 (adaptive)`}
                </div>
              ) : null}
              <div className="flex items-end gap-2 rounded-[26px] border border-border bg-card px-4 py-2 shadow-sm transition-shadow focus-within:border-foreground/25 focus-within:ring-2 focus-within:ring-foreground/10">
                <textarea
                  ref={textareaRef}
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={onKeyDown}
                  rows={1}
                  autoFocus
                  disabled={!active && pendingMode == null}
                  placeholder={
                    !active && pendingMode == null
                      ? 'Pick Fast or Complex above to start'
                      : 'Describe a feature or fix in your own words'
                  }
                  className="max-h-40 flex-1 resize-none overflow-y-auto bg-transparent py-1.5 text-[15px] leading-6 text-foreground placeholder:text-muted-foreground focus:outline-none disabled:cursor-not-allowed"
                />
                {busy ? (
                  <button
                    type="button"
                    onClick={stop}
                    className="mb-0.5 inline-flex size-8 flex-none items-center justify-center rounded-full bg-foreground text-background transition-opacity hover:opacity-85"
                    aria-label="Stop generating"
                    title="Stop generating"
                  >
                    <Square className="size-3 fill-current" />
                  </button>
                ) : (
                  <button
                    type="submit"
                    disabled={!draft.trim() || (!active && pendingMode == null)}
                    className="mb-0.5 inline-flex size-8 flex-none items-center justify-center rounded-full bg-foreground text-background transition-opacity hover:opacity-85 disabled:opacity-30"
                    aria-label="Send"
                    title="Send (⏎)"
                  >
                    <ArrowUp className="size-4" strokeWidth={2.5} />
                  </button>
                )}
              </div>
              <div className="mt-1.5 text-center text-[11px] text-muted-foreground/60">
                Enter to send · Shift+Enter for a new line
              </div>
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
 *  `resources/logo.svg`) in white on a gradient tile. Used only in the empty
 *  state now; transcript turns render flat, ChatGPT-style. */
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
        className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[13px] font-medium text-foreground/90 transition-colors hover:bg-accent disabled:opacity-50"
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
          className="absolute left-0 top-full z-30 mt-1 w-72 rounded-xl border border-border bg-card p-1 shadow-lg"
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
        className="inline-flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-[13px] font-medium text-foreground/90 transition-colors hover:bg-accent disabled:opacity-50"
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span>{current.label}</span>
        <ChevronDown
          className={cn('size-3.5 text-muted-foreground transition-transform', open && 'rotate-180')}
        />
      </button>
      {open ? (
        <div
          role="listbox"
          className="absolute left-0 top-full z-30 mt-1 w-64 rounded-xl border border-border bg-card p-1 shadow-lg"
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
        'inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[13px] font-medium transition-colors disabled:opacity-50',
        on ? 'bg-primary/10 text-primary' : 'text-muted-foreground hover:bg-accent'
      )}
    >
      <Brain className="size-3.5" /> Thinking
    </button>
  )
}

/** Collapsible reasoning trace shown above an assistant answer — a quiet text
 *  disclosure, ChatGPT's "thought for…" style. Open while the reply streams,
 *  then auto-collapses — unless the user has toggled it. */
function Reasoning({ text, streaming }: { text: string; streaming: boolean }) {
  const [userOpen, setUserOpen] = useState<boolean | null>(null)
  const open = userOpen ?? streaming
  return (
    <div className="mb-2.5">
      <button
        type="button"
        onClick={() => setUserOpen(!open)}
        className="inline-flex items-center gap-1.5 text-[12.5px] font-medium text-muted-foreground transition-colors hover:text-foreground"
      >
        <span>{streaming && !text ? 'Thinking…' : 'Reasoning'}</span>
        {streaming ? <Loader2 className="size-3 animate-spin" /> : null}
        <ChevronDown className={cn('size-3.5 transition-transform', open && 'rotate-180')} />
      </button>
      {open && text ? (
        <div className="mt-2 whitespace-pre-wrap border-l-2 border-border pl-3.5 text-[13px] leading-relaxed text-muted-foreground [overflow-wrap:anywhere]">
          {text}
        </div>
      ) : null}
    </div>
  )
}

/** A dismissible inline error strip for the transcript. */
/** Amber sibling of ErrorBanner for the blind-context warning (spec 009
 *  #361). No dismiss: the flag reflects live server state and clears itself on
 *  the next grounded send — hiding it by hand would hide a real problem. */
function WarningBanner({ text }: { text: string }) {
  return (
    <div className="flex items-start gap-2 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-[12.5px] text-amber-400">
      <span className="min-w-0 flex-1 [overflow-wrap:anywhere]">{text}</span>
    </div>
  )
}

function ErrorBanner({ text, onDismiss }: { text: string; onDismiss: () => void }) {
  return (
    <div className="flex items-start gap-2 rounded-lg border border-red-500/40 bg-red-500/10 px-3 py-2 text-[12.5px] text-red-400">
      <span className="min-w-0 flex-1 [overflow-wrap:anywhere]">{text}</span>
      <button
        type="button"
        onClick={onDismiss}
        aria-label="Dismiss error"
        className="flex-none rounded p-0.5 transition-colors hover:bg-red-500/20"
      >
        <X className="size-3.5" />
      </button>
    </div>
  )
}

/** One chat turn, ChatGPT-style: user turns are a right-aligned quiet bubble;
 *  assistant turns are flat markdown on the page background — no avatar, no
 *  name eyebrow, no box — with the optional reasoning trace above. A
 *  filing-summary turn renders the clickable issues card instead. Memoized so
 *  a streaming reply re-renders only the trailing turn per token, not every
 *  completed turn's markdown. */
const Turn = memo(function Turn({
  turn,
  streaming,
  onOpenIssue
}: {
  turn: StoredTurn
  streaming: boolean
  onOpenIssue: (filed: FiledResult, issue: FiledResult['issues'][number]) => void
}) {
  if (turn.role === 'user') {
    return (
      <div className="flex justify-end">
        <div className="max-w-[85%] whitespace-pre-wrap rounded-3xl rounded-br-lg bg-accent px-4 py-2.5 text-[15px] leading-relaxed text-foreground [overflow-wrap:anywhere]">
          {turn.content}
        </div>
      </div>
    )
  }
  return (
    <div className="min-w-0">
      {turn.thinking ? <Reasoning text={turn.thinking} streaming={streaming} /> : null}
      {turn.filed ? (
        <FiledCard filed={turn.filed} onOpenIssue={onOpenIssue} />
      ) : turn.content ? (
        <CommentMarkdown
          content={turn.content}
          variant="compact"
          className="text-[15px] leading-7 text-foreground"
        />
      ) : streaming && !turn.thinking ? (
        <span
          className="mt-1 inline-block size-2.5 animate-pulse rounded-full bg-foreground/80"
          aria-label="Generating…"
        />
      ) : null}
    </div>
  )
})

/** The filing-summary card: what was created, where — and the bridge onward.
 *  Clicking a row opens the Tasks board with that issue's detail; the trailing
 *  arrow opens the tracker in the system browser (via the native opener —
 *  `target="_blank"` anchors are dead inside the Tauri webview). Failures are
 *  listed with their reasons, never hidden behind a count. */
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
    <div className="overflow-hidden rounded-2xl border border-border bg-card/60">
      <div className="flex items-center gap-2 border-b border-border/60 px-4 py-2.5">
        {filed.provider === 'linear' ? (
          <Check className="size-3.5 text-primary" />
        ) : (
          <Github className="size-3.5 text-muted-foreground" />
        )}
        <span className="min-w-0 truncate text-[13px] font-medium">
          {n > 0 ? `Filed to ${destination}` : `Nothing filed to ${destination}`}
        </span>
        <span className="ml-auto flex-none text-[11px] text-muted-foreground">
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
                  <span className="inline-flex flex-none items-center gap-1 text-[11px] text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100">
                    <Columns3 className="size-3.5" /> board
                  </span>
                </button>
                {issue.url ? (
                  <button
                    type="button"
                    onClick={() => void api.shell.openUrl(issue.url)}
                    title={`Open on ${providerLabel(filed.provider)}`}
                    aria-label={`Open ${issue.title} on ${providerLabel(filed.provider)}`}
                    className="flex flex-none items-center border-l border-border/60 px-3 text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground"
                  >
                    <ArrowUpRight className="size-3.5" />
                  </button>
                ) : null}
              </li>
            )
          })}
        </ul>
      ) : null}
      {filed.failed.length > 0 ? (
        <div className="border-t border-border/60 px-4 py-2.5">
          <div className="mb-1 text-[11px] font-medium text-red-400">
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

/** Empty conversation hero — a centered greeting with example prompt chips. */
function EmptyState({ onPick }: { onPick: (text: string) => void }) {
  const examples = [
    'Fix the session list losing scroll position',
    'Let users star a worktree to pin it',
    'Add a shortcut to jump between sessions'
  ]
  return (
    <div className="flex flex-col items-center px-4 pb-8 pt-20 text-center">
      <AssistantAvatar className="size-10" />
      <h2 className="mt-5 text-[26px] font-semibold tracking-tight text-foreground">
        What should we build?
      </h2>
      <p className="mt-2 max-w-[26rem] text-[14px] leading-relaxed text-muted-foreground">
        Pick Fast or Complex below, then describe the feature. I'll draft GitHub issues you can
        review before anything is filed.
      </p>
      <div className="mt-7 flex flex-wrap justify-center gap-2">
        {examples.map((ex) => (
          <button
            key={ex}
            type="button"
            onClick={() => onPick(ex)}
            className="rounded-full border border-border px-4 py-2 text-[13px] text-foreground/85 transition-colors hover:bg-accent"
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
