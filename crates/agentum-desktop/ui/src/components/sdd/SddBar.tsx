// SDD bar (issue #313): quick-inject buttons + the Loop toggle, rendered under
// the terminal for agent tabs. The playbooks are SERVER-owned (agentum-server
// embeds them and serves them over MCP); a button injects a short bootstrap
// line into the pane telling the agent to fetch the playbook via the
// `agentum_sdd` MCP tool — agent-agnostic by construction. The Loop toggle is
// server state: we render whatever `/sdd/loop` + the `sdd.loop.*` events say,
// so the rainbow survives reloads and reflects loops started by anyone.
import { useCallback, useEffect, useMemo, useState } from 'react'
import { FileText, ListChecks, MessagesSquare, Repeat2, StepForward } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { useAppStore } from '@/store'
import { useTabAgent } from '@/lib/use-tab-agent'
import type { TerminalTab } from '../../../../shared/types'
import {
  getSddLoop,
  injectSddPlaybook,
  listSddPlaybooks,
  setSddLoop,
  type SddLoopState,
  type SddPlaybook
} from '@/runtime/sdd-client'
import { subscribeServerEvents } from '@/runtime/server-events-bus'

/** The row's quick actions — plain labels, not slash commands (the command
 *  names stay an implementation detail of the server registry). */
const SDD_BUTTONS: { playbook: string; label: string; icon: LucideIcon }[] = [
  { playbook: 'sdd-spec', label: 'Spec', icon: FileText },
  { playbook: 'sdd-spec-socratic', label: 'Spec Socratic', icon: MessagesSquare },
  { playbook: 'sdd-orchestrate', label: 'Continue', icon: StepForward },
  { playbook: 'sdd-status', label: 'Status', icon: ListChecks }
]

const PILL_CLASS =
  'inline-flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-card px-3 py-1 text-[12.5px] font-medium text-foreground transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-40'

const INACTIVE_LOOP: SddLoopState = { active: false, step: 0, maxSteps: 0 }

/** The server session id behind an agent tab: server-session panes register a
 *  `server:<sessionId>:<leafId>` ptyId (see server-pane-connection.ts) — the
 *  reactive twin of `resolveServerSessionId`, so the bar enables itself the
 *  moment the pane binds. */
function useServerSessionId(tabId: string): string | null {
  const ptyIds = useAppStore((s) => s.ptyIdsByTabId[tabId])
  return useMemo(() => {
    const match = (ptyIds ?? []).find((p) => p.startsWith('server:'))
    return match ? (match.split(':')[1] ?? null) : null
  }, [ptyIds])
}

function PlaybookPreviewModal({
  playbook,
  onCancel,
  onConfirm
}: {
  playbook: SddPlaybook
  onCancel: () => void
  onConfirm: () => void
}): React.JSX.Element {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      role="dialog"
      aria-modal="true"
      aria-label={`Preview ${playbook.title}`}
      onClick={onCancel}
    >
      <div
        className="flex max-h-[70vh] w-full max-w-xl flex-col overflow-hidden rounded-lg border border-border bg-card shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-border/60 px-4 py-3">
          <span className="text-sm font-semibold text-foreground">{playbook.title}</span>
          <span className="rounded-full border border-border px-2 py-0.5 text-[10px] uppercase tracking-wider text-muted-foreground">
            server-side
          </span>
          <span className="ml-auto truncate text-xs text-muted-foreground">{playbook.name}</span>
        </div>
        <p className="border-b border-border/60 px-4 py-2 text-xs text-muted-foreground">
          {playbook.description}
        </p>
        <pre className="min-h-0 flex-1 overflow-y-auto whitespace-pre-wrap px-4 py-3 font-mono text-xs leading-relaxed text-foreground/90">
          {playbook.body}
        </pre>
        <div className="flex items-center gap-3 border-t border-border/60 px-4 py-3">
          <p className="min-w-0 flex-1 text-xs text-muted-foreground">
            Injects a short bootstrap into the pane — the agent fetches this playbook itself via
            the agentum MCP (full text for non-MCP tools).
          </p>
          <button className={PILL_CLASS} onClick={onCancel}>
            Cancel
          </button>
          <button
            className={`${PILL_CLASS} border-primary/60 bg-primary text-primary-foreground hover:bg-primary/90`}
            onClick={onConfirm}
          >
            Inject →
          </button>
        </div>
      </div>
    </div>
  )
}

/**
 * Visibility gate for the SDD bar: show it whenever the tab is ACTUALLY
 * running an agent, resolved from live signals (foreground process, pane
 * title, lifecycle hooks, launchAgent) via `useTabAgent` — NOT the raw
 * `tab.launchAgent` field, which is only stamped by the agent quick-launcher
 * and absent on attached sessions, persisted tabs, and manually-started
 * agents (the v0.72.0 "where are the buttons?" bug).
 */
export function SddBarGate({ tab }: { tab: TerminalTab }): React.JSX.Element | null {
  const agent = useTabAgent(tab)
  if (!agent) {
    return null
  }
  return <SddBar tabId={tab.id} />
}

export function SddBar({ tabId }: { tabId: string }): React.JSX.Element {
  const sessionId = useServerSessionId(tabId)
  const [playbooks, setPlaybooks] = useState<SddPlaybook[] | null>(null)
  const [preview, setPreview] = useState<SddPlaybook | null>(null)
  const [loop, setLoop] = useState<SddLoopState>(INACTIVE_LOOP)
  const [loopPending, setLoopPending] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  // Authoritative loop state: snapshot on bind, then live `sdd.loop.*` deltas.
  useEffect(() => {
    if (!sessionId) {
      setLoop(INACTIVE_LOOP)
      return
    }
    let cancelled = false
    const refresh = (): void => {
      void getSddLoop(sessionId)
        .then((st) => {
          if (!cancelled) {
            setLoop(st)
          }
        })
        .catch(() => {})
    }
    const unsubscribe = subscribeServerEvents({
      onEvent: (ev) => {
        if (typeof ev.kind !== 'string' || !ev.kind.startsWith('sdd.loop.')) {
          return
        }
        if (ev.session_id !== sessionId) {
          return
        }
        if (ev.kind === 'sdd.loop.stopped') {
          setLoop(INACTIVE_LOOP)
          const reason = (ev.payload as { reason?: string } | undefined)?.reason
          if (reason && reason !== 'toggled_off') {
            setNotice(`Loop ended: ${reason.replace(/_/g, ' ')}`)
          }
        } else {
          // started/step both mean "active"; refetch for the exact counters.
          refresh()
        }
      },
      // Reconnects can miss events — refetch the snapshot each (re)open.
      onOpen: refresh
    })
    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [sessionId])

  // The "Loop ended: …" notice is transient by design — it explains a rainbow
  // that just went out, then gets out of the way.
  useEffect(() => {
    if (!notice) {
      return
    }
    const t = window.setTimeout(() => setNotice(null), 6000)
    return () => window.clearTimeout(t)
  }, [notice])

  const openPreview = useCallback(
    (name: string): void => {
      const cached = playbooks?.find((p) => p.name === name)
      if (cached) {
        setPreview(cached)
        return
      }
      void listSddPlaybooks()
        .then((all) => {
          setPlaybooks(all)
          setPreview(all.find((p) => p.name === name) ?? null)
        })
        .catch(() => setNotice('Could not load SDD playbooks'))
    },
    [playbooks]
  )

  const confirmInject = useCallback((): void => {
    if (!sessionId || !preview) {
      return
    }
    const { name, title } = preview
    setPreview(null)
    void injectSddPlaybook(sessionId, name)
      .then(({ mode }) =>
        setNotice(mode === 'bootstrap' ? `${title} sent via MCP` : `${title} sent (full text)`)
      )
      .catch(() => setNotice(`Could not inject ${title}`))
  }, [sessionId, preview])

  const toggleLoop = useCallback((): void => {
    if (!sessionId || loopPending) {
      return
    }
    setLoopPending(true)
    void setSddLoop(sessionId, !loop.active)
      .then(setLoop)
      .catch(() => setNotice('Could not toggle the SDD loop'))
      .finally(() => setLoopPending(false))
  }, [sessionId, loop.active, loopPending])

  const disabled = sessionId === null
  return (
    <div className="flex shrink-0 items-center gap-2 border-t border-border/60 bg-background px-3 py-1.5">
      <span className="text-[10px] font-semibold uppercase tracking-widest text-muted-foreground">
        SDD
      </span>
      {SDD_BUTTONS.map(({ playbook, label, icon: Icon }) => (
        <button
          key={playbook}
          className={PILL_CLASS}
          disabled={disabled}
          title={disabled ? 'Waiting for the agent session to connect' : undefined}
          onClick={() => openPreview(playbook)}
        >
          <Icon className="size-3.5" />
          {label}
        </button>
      ))}
      <span className="ml-auto min-w-0 truncate text-xs text-muted-foreground">{notice}</span>
      <button
        className={`${PILL_CLASS} ${loop.active ? 'sdd-rainbow-border' : ''}`}
        disabled={disabled || loopPending}
        aria-pressed={loop.active}
        title={
          loop.active
            ? `SDD loop running (step ${loop.step}/${loop.maxSteps}) — click to stop`
            : 'Run the SDD loop on this session (autonomous orchestrate until done)'
        }
        onClick={toggleLoop}
      >
        <Repeat2 className={`size-3.5 ${loop.active ? 'text-fuchsia-400' : ''}`} />
        <span className={loop.active ? 'sdd-rainbow-text' : ''}>
          {loop.active ? `Loop ${loop.step}/${loop.maxSteps}` : 'Loop'}
        </span>
      </button>
      {preview && (
        <PlaybookPreviewModal
          playbook={preview}
          onCancel={() => setPreview(null)}
          onConfirm={confirmInject}
        />
      )}
    </div>
  )
}
