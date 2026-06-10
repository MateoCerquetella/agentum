// "Remote tmux" sidebar section: when the active project lives on an SSH host,
// list the tmux sessions already running there (the user's own sessions, not
// agentum-managed ones) and open any of them as a live terminal tab. The tab
// streams through the embedded server (pipe-pane + tail over one SSH channel,
// same as managed sessions); closing it never kills the underlying session.
//
// Fetching is on-demand only — once when the active SSH project changes and on
// the manual refresh button. No background polling: each fetch is an SSH round
// trip and idle projects shouldn't keep a remote connection chatty.
import React, { useEffect, useMemo, useState } from 'react'
import { ChevronDown, Loader2, RefreshCw, SquareTerminal } from 'lucide-react'
import { useAppStore } from '@/store'
import { getRepoMapFromState, getWorktreeMapFromState } from '@/store/selectors'
import {
  attachHostTmuxSession,
  resolveServerHostIdForConnection,
  type DiscoveredTmuxSession
} from '@/runtime/server-host-client'
import { cn } from '@/lib/utils'

function sessionSubtitle(s: DiscoveredTmuxSession): string {
  const command = s.panes[0]?.command ?? ''
  const panes = s.panes.length > 1 ? ` · ${s.panes.length} panes` : ''
  return `${command}${panes}`
}

function RemoteTmuxSection(): React.JSX.Element | null {
  const activeWorktreeId = useAppStore((s) => s.activeWorktreeId)
  // Cached map selectors return stable references, so these subscriptions
  // don't re-render on unrelated store updates.
  const repo = useAppStore((s) => {
    const worktree = activeWorktreeId
      ? getWorktreeMapFromState(s).get(activeWorktreeId)
      : undefined
    return worktree ? (getRepoMapFromState(s).get(worktree.repoId) ?? null) : null
  })
  const connectionId = repo?.connectionId ?? null
  const hostKey = connectionId ? `ssh:${connectionId}` : null
  const remoteTmux = useAppStore((s) => (hostKey ? s.remoteTmuxByHostKey[hostKey] : undefined))
  const fetchRemoteTmuxSessions = useAppStore((s) => s.fetchRemoteTmuxSessions)
  const [collapsed, setCollapsed] = useState(false)
  const [showAll, setShowAll] = useState(false)
  const [attachingName, setAttachingName] = useState<string | null>(null)
  const [attachError, setAttachError] = useState<string | null>(null)

  const repoPath = repo?.path

  useEffect(() => {
    if (connectionId) {
      void fetchRemoteTmuxSessions(connectionId, repoPath)
    }
  }, [connectionId, repoPath, fetchRemoteTmuxSessions])

  const sessions = remoteTmux?.sessions ?? []
  const { related, other } = useMemo(() => {
    return {
      related: sessions.filter((s) => s.related),
      other: sessions.filter((s) => !s.related)
    }
  }, [sessions])

  // Local projects (or no active project) have no remote tmux to show.
  if (!connectionId || !hostKey) {
    return null
  }

  const visible = showAll ? [...related, ...other] : related.length > 0 ? related : other
  const hiddenCount = showAll ? 0 : related.length > 0 ? other.length : 0

  const handleAttach = async (name: string): Promise<void> => {
    if (attachingName || !activeWorktreeId) {
      return
    }
    setAttachingName(name)
    setAttachError(null)
    try {
      const hostId = await resolveServerHostIdForConnection(connectionId)
      if (!hostId) {
        throw new Error('host is not registered with the server')
      }
      const session = await attachHostTmuxSession(hostId, name)
      const store = useAppStore.getState()
      // Re-click focuses the existing tab instead of duplicating the stream.
      const existing = (store.tabsByWorktree[activeWorktreeId] ?? []).find(
        (t) => t.serverSessionId === session.id
      )
      if (existing) {
        store.setActiveTab(existing.id)
        return
      }
      const tab = store.createTab(activeWorktreeId, undefined, undefined, {
        activate: true,
        recordInteraction: true,
        // Force the server-backed pane path; a local PTY can't render a
        // remote tmux session.
        persistTmux: true,
        serverSessionId: session.id
      })
      store.setTabCustomTitle(tab.id, `tmux: ${name}`)
    } catch (err) {
      setAttachError(err instanceof Error ? err.message : String(err))
    } finally {
      setAttachingName(null)
    }
  }

  return (
    <div className="shrink-0 border-t border-sidebar-border px-2 py-1.5">
      <div className="flex h-7 w-full items-center gap-1.5 px-1">
        <button
          type="button"
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-1.5 text-left"
          onClick={() => setCollapsed((c) => !c)}
          aria-expanded={!collapsed}
        >
          <ChevronDown
            className={cn(
              'size-3.5 shrink-0 text-muted-foreground transition-transform',
              collapsed && '-rotate-90'
            )}
          />
          <SquareTerminal className="size-4 shrink-0 text-muted-foreground" />
          <span className="truncate text-xs font-semibold text-foreground">Remote tmux</span>
          {sessions.length > 0 && (
            <span className="inline-flex items-center rounded-full bg-sidebar-accent px-1.5 py-0.5 text-[11px] text-muted-foreground">
              {sessions.length}
            </span>
          )}
        </button>
        <button
          type="button"
          title="Refresh remote tmux sessions"
          className="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
          onClick={() => void fetchRemoteTmuxSessions(connectionId, repoPath)}
        >
          {remoteTmux?.status === 'loading' ? (
            <Loader2 className="size-3 animate-spin" />
          ) : (
            <RefreshCw className="size-3" />
          )}
        </button>
      </div>

      {!collapsed && (
        <div className="max-h-48 overflow-y-auto">
          {remoteTmux?.status === 'error' && (
            <div className="px-2 py-1 text-[11px] text-muted-foreground">
              Could not reach host: {remoteTmux.error}
            </div>
          )}
          {remoteTmux?.status === 'ready' && sessions.length === 0 && (
            <div className="px-2 py-1 text-[11px] text-muted-foreground">
              No tmux sessions on this host
            </div>
          )}
          {attachError && (
            <div className="px-2 py-1 text-[11px] text-destructive">{attachError}</div>
          )}
          {visible.map((s) => (
            <button
              key={s.name}
              type="button"
              title={`Open tmux session "${s.name}" (${sessionSubtitle(s)})`}
              className="group flex h-7 w-full cursor-pointer items-center gap-1.5 rounded px-1 pl-6 text-left hover:bg-sidebar-accent"
              disabled={attachingName !== null}
              onClick={() => void handleAttach(s.name)}
            >
              <span
                title={s.attached ? 'A tmux client is attached' : 'Detached'}
                className={cn(
                  'size-1.5 shrink-0 rounded-full',
                  s.attached ? 'bg-emerald-500' : 'bg-muted-foreground/40'
                )}
              />
              <span className="truncate text-xs text-foreground">{s.name}</span>
              <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
                {sessionSubtitle(s)}
              </span>
              {attachingName === s.name && (
                <Loader2 className="size-3 shrink-0 animate-spin text-muted-foreground" />
              )}
            </button>
          ))}
          {hiddenCount > 0 && (
            <button
              type="button"
              className="flex h-6 w-full cursor-pointer items-center rounded px-1 pl-6 text-[11px] text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
              onClick={() => setShowAll(true)}
            >
              Show all ({sessions.length})
            </button>
          )}
          {showAll && other.length > 0 && related.length > 0 && (
            <button
              type="button"
              className="flex h-6 w-full cursor-pointer items-center rounded px-1 pl-6 text-[11px] text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
              onClick={() => setShowAll(false)}
            >
              Show related only
            </button>
          )}
        </div>
      )}
    </div>
  )
}

export default React.memo(RemoteTmuxSection)
