// Host-level tmux session browser.
// Panel 1 (list): all sessions, kill-inactive bulk action.
// Panel 2 (detail): opens when user clicks View — shows session in a terminal
//   tab and provides a ← back button to return to the list.
import React, { useCallback, useEffect, useState } from 'react'
import { Loader2, RefreshCw, SquareTerminal, Trash2 } from 'lucide-react'
import {
  Dialog,
  DialogContent,
} from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/store'
import {
  killHostTmuxSession,
  attachHostTmuxSession,
  resolveServerHostIdForHostKey,
  listHostTmuxSessions,
  type DiscoveredTmuxSession
} from '@/runtime/server-host-client'
import { pickWorktreeForHost } from './worktree-list-groups'

function relativeTime(epochSecs: number): string {
  const diff = Math.floor(Date.now() / 1000) - epochSecs
  if (diff < 60) return 'just now'
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  return `${Math.floor(diff / 86400)}d ago`
}

export interface TmuxSessionsModalHost {
  key: string
  label: string
  kind: 'local' | 'ssh'
}

interface SessionRowProps {
  session: DiscoveredTmuxSession
  isManaged: boolean
  opening: boolean
  confirming: boolean
  onView: () => void
  onKillRequest: () => void
  onKillCancel: () => void
  onKillConfirm: () => void
}

function SessionRow({
  session: s,
  isManaged,
  opening,
  confirming,
  onView,
  onKillRequest,
  onKillCancel,
  onKillConfirm,
}: SessionRowProps) {
  const canKill = !s.attached

  return (
    <div className={cn(
      'group rounded-lg border border-transparent px-3 py-2.5 transition-colors',
      confirming ? 'border-destructive/30 bg-destructive/5' : 'hover:bg-sidebar-accent/40'
    )}>
      {/* row 1: status + name */}
      <div className="flex items-center gap-2">
        <span
          title={s.attached ? 'Active' : 'Inactive'}
          className={cn('size-2 shrink-0 rounded-full', s.attached ? 'bg-emerald-500' : 'bg-zinc-400/50')}
        />
        <span className="min-w-0 flex-1 truncate text-sm font-semibold text-foreground">
          {s.name}
        </span>
      </div>

      {/* row 2: meta + actions */}
      <div className="mt-1.5 flex items-center gap-2 pl-4">
        <span className="text-[11px] text-muted-foreground">
          {s.panes[0]?.command ?? ''}
          {s.panes.length > 1 ? ` · ${s.panes.length} panes` : ''}
        </span>
        {isManaged && (
          <span className="rounded bg-primary/10 px-1 py-0.5 text-[10px] font-medium text-primary">
            agentum
          </span>
        )}
        {s.created_at != null && (
          <span className="text-[11px] text-muted-foreground">{relativeTime(s.created_at)}</span>
        )}

        <div className="ml-auto flex items-center gap-1">
          {!confirming ? (
            <>
              {/* VIEW — always visible */}
              <button
                type="button"
                title="View in terminal"
                disabled={opening}
                onClick={onView}
                className="flex items-center gap-1 rounded px-2 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary disabled:opacity-40"
              >
                {opening
                  ? <Loader2 className="size-3 animate-spin" />
                  : <SquareTerminal className="size-3" />}
                View
              </button>
              {/* KILL — only for inactive */}
              {canKill && (
                <button
                  type="button"
                  title="Kill session"
                  onClick={onKillRequest}
                  className="flex items-center gap-1 rounded px-2 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                >
                  <Trash2 className="size-3" />
                  Kill
                </button>
              )}
            </>
          ) : (
            <>
              <span className="text-[11px] text-destructive">Kill this session?</span>
              <button
                type="button"
                onClick={onKillConfirm}
                className="rounded bg-destructive px-2 py-1 text-[11px] font-medium text-destructive-foreground transition-opacity hover:opacity-90"
              >
                Yes, kill
              </button>
              <button
                type="button"
                onClick={onKillCancel}
                className="rounded px-2 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-sidebar-accent"
              >
                Cancel
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}

interface TmuxSessionsModalProps {
  host: TmuxSessionsModalHost | null
  onClose: () => void
}

export function TmuxSessionsModal({ host, onClose }: TmuxSessionsModalProps): React.JSX.Element {
  const [hostId, setHostId] = useState<string | null>(null)
  const [sessions, setSessions] = useState<DiscoveredTmuxSession[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [openingName, setOpeningName] = useState<string | null>(null)
  const [openError, setOpenError] = useState<string | null>(null)
  const [confirmingKill, setConfirmingKill] = useState<string | null>(null)
  const [killingAll, setKillingAll] = useState(false)

  useEffect(() => {
    if (!host) { setHostId(null); setSessions([]); return }
    let cancelled = false
    resolveServerHostIdForHostKey(host.key)
      .then((id) => { if (!cancelled) setHostId(id) })
      .catch(() => { if (!cancelled) setError('Could not resolve host') })
    return () => { cancelled = true }
  }, [host?.key])

  const fetchSessions = useCallback(async (id: string): Promise<void> => {
    setLoading(true)
    setError(null)
    try {
      const result = await listHostTmuxSessions(id, { all: true })
      setSessions(result)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (hostId) void fetchSessions(hostId)
  }, [hostId, fetchSessions])

  const handleView = useCallback(async (sessionName: string): Promise<void> => {
    if (!hostId || !host) return
    setOpeningName(sessionName)
    setOpenError(null)
    try {
      const store = useAppStore.getState()
      const allWorktrees = Object.values(store.worktreesByRepo).flat()
      const repoMap = new Map((store.repos as import('@/shared/types').Repo[]).map((r) => [r.id, r]))
      const worktree = pickWorktreeForHost(host.key, allWorktrees, repoMap, store.activeWorktreeId)
      if (!worktree) throw new Error('No workspace on this host to open the session in')
      const serverSession = await attachHostTmuxSession(hostId, sessionName)
      const existing = (store.tabsByWorktree[worktree.id] ?? []).find(
        (t) => t.serverSessionId === serverSession.id
      )
      store.setActiveWorktree(worktree.id)
      if (existing) { store.setActiveTab(existing.id); onClose(); return }
      const tab = store.createTab(worktree.id, undefined, undefined, {
        activate: true, recordInteraction: true, persistTmux: true, serverSessionId: serverSession.id
      })
      store.setTabCustomTitle(tab.id, `tmux: ${sessionName}`)
      onClose()
    } catch (err) {
      setOpenError(err instanceof Error ? err.message : String(err))
    } finally {
      setOpeningName(null)
    }
  }, [hostId, host, onClose])

  const handleKillConfirm = useCallback(async (name: string): Promise<void> => {
    if (!hostId) return
    try {
      await killHostTmuxSession(hostId, name)
      setSessions((prev) => prev.filter((s) => s.name !== name))
    } catch (err) {
      setOpenError(err instanceof Error ? err.message : String(err))
    } finally {
      setConfirmingKill(null)
    }
  }, [hostId])

  const inactiveSessions = sessions.filter((s) => !s.attached)

  const handleKillAllInactive = async (): Promise<void> => {
    if (!hostId || inactiveSessions.length === 0) return
    setKillingAll(true)
    await Promise.allSettled(inactiveSessions.map((s) => killHostTmuxSession(hostId, s.name)))
    setKillingAll(false)
    void fetchSessions(hostId)
  }

  return (
    <Dialog open={host !== null} onOpenChange={(open) => { if (!open) onClose() }}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-[700px]">

        {/* ── header ── */}
        <div className="flex shrink-0 items-center gap-2 border-b px-4 py-3">
          <SquareTerminal className="size-4 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1 truncate text-sm font-semibold">
            Tmux — {host?.label ?? ''}
          </span>

          {/* kill all inactive */}
          {inactiveSessions.length > 0 && (
            <button
              type="button"
              title="Kill all inactive sessions"
              disabled={killingAll}
              onClick={() => void handleKillAllInactive()}
              className="flex items-center gap-1.5 rounded-md border border-destructive/30 px-2.5 py-1 text-[11px] font-medium text-destructive/80 transition-colors hover:bg-destructive/10 hover:text-destructive disabled:opacity-40"
            >
              {killingAll ? <Loader2 className="size-3 animate-spin" /> : <Trash2 className="size-3" />}
              Kill inactive ({inactiveSessions.length})
            </button>
          )}

          {/* refresh — mr-8 to clear the Dialog's absolute X button */}
          <button
            type="button"
            title="Refresh"
            disabled={loading || !hostId}
            onClick={() => { if (hostId) void fetchSessions(hostId) }}
            className="mr-8 flex size-7 items-center justify-center rounded text-muted-foreground hover:bg-sidebar-accent hover:text-foreground disabled:opacity-40"
          >
            {loading ? <Loader2 className="size-3.5 animate-spin" /> : <RefreshCw className="size-3.5" />}
          </button>

        </div>

        {/* ── session count ── */}
        {sessions.length > 0 && (
          <div className="shrink-0 border-b px-4 py-2 text-[11px] text-muted-foreground">
            {sessions.length} session{sessions.length !== 1 ? 's' : ''} ·{' '}
            {sessions.filter(s => s.attached).length} active ·{' '}
            {inactiveSessions.length} inactive
          </div>
        )}

        {/* ── body ── */}
        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="flex flex-col gap-1 px-3 py-3">
            {error && <p className="rounded-md bg-destructive/10 px-3 py-2 text-[11px] text-destructive">{error}</p>}
            {openError && <p className="rounded-md bg-destructive/10 px-3 py-1 text-[11px] text-destructive">{openError}</p>}
            {!loading && !error && sessions.length === 0 && (
              <p className="py-6 text-center text-[11px] text-muted-foreground">No tmux sessions on this host</p>
            )}
            {sessions.map((s) => (
              <SessionRow
                key={s.name}
                session={s}
                isManaged={s.name.startsWith('agentum-')}
                opening={openingName === s.name}
                confirming={confirmingKill === s.name}
                onView={() => void handleView(s.name)}
                onKillRequest={() => setConfirmingKill(s.name)}
                onKillCancel={() => setConfirmingKill(null)}
                onKillConfirm={() => void handleKillConfirm(s.name)}
              />
            ))}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
