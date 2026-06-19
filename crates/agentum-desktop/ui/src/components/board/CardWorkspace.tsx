// CardWorkspace — the live drill-in for a started board card (Phase 2, #48).
//
// A card's "Start" spawns a server-backed agent session (per-card worktree +
// the shared launch path). This view mounts that session's tmux pane live so
// the user watches the agent work, reusing the same streaming primitive the
// desktop's terminal panes use (`bindServerSessionTerminal`) rather than the
// full TerminalPane tab machinery — the pane lives in tmux on the server, so
// this is a thin xterm bound to its byte stream.
import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { ArrowLeft, RefreshCw } from 'lucide-react'
import '@xterm/xterm/css/xterm.css'

import { buildDefaultTerminalOptions } from '@/lib/pane-manager/pane-terminal-options'
import {
  type ServerSessionTerminalBinding,
  bindServerSessionTerminal
} from '@/runtime/server-session-terminal'

type CardWorkspaceProps = {
  /** The agentum-server session id bound to the card (its tmux pane). */
  sessionId: string
  /** Card key + title, shown in the breadcrumb. */
  cardKey: string
  cardTitle: string
  /** Return to the board. */
  onBack: () => void
}

export default function CardWorkspace({
  sessionId,
  cardKey,
  cardTitle,
  onBack
}: CardWorkspaceProps) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const bindingRef = useRef<ServerSessionTerminalBinding | null>(null)

  useEffect(() => {
    const container = containerRef.current
    if (!container) {
      return
    }

    const terminal = new Terminal(buildDefaultTerminalOptions())
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(container)
    try {
      fit.fit()
    } catch {
      // Fitting before layout settles can throw; the ResizeObserver below
      // re-fits once the container has real dimensions.
    }

    // Keep the pane sized to its container — xterm forwards the new dims to the
    // server pane via the binding's onResize, so the agent re-wraps correctly.
    const ro = new ResizeObserver(() => {
      try {
        fit.fit()
      } catch {
        /* container detached mid-teardown — ignore */
      }
    })
    ro.observe(container)

    let disposed = false
    void bindServerSessionTerminal(sessionId, terminal).then((binding) => {
      if (disposed) {
        binding.dispose()
        return
      }
      bindingRef.current = binding
    })

    return () => {
      disposed = true
      ro.disconnect()
      bindingRef.current?.dispose()
      bindingRef.current = null
      terminal.dispose()
    }
  }, [sessionId])

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-background text-foreground">
      <div className="flex items-center gap-2 border-b border-border/60 px-3 py-2">
        <button
          type="button"
          onClick={onBack}
          className="flex items-center gap-1 rounded-md px-2 py-1 text-[12px] text-foreground/70 hover:bg-foreground/8"
        >
          <ArrowLeft className="size-3.5" />
          Board
        </button>
        <span className="text-foreground/30">/</span>
        <span className="rounded bg-foreground/10 px-1.5 py-0.5 font-mono text-[11px] text-foreground/60">
          {cardKey}
        </span>
        <span className="truncate text-[13px] font-medium">{cardTitle}</span>
        <button
          type="button"
          onClick={() => bindingRef.current?.forceRedraw()}
          aria-label="Force redraw"
          title="Force redraw"
          className="ml-auto flex items-center gap-1 rounded-md px-2 py-1 text-[12px] text-foreground/60 hover:bg-foreground/8"
        >
          <RefreshCw className="size-3.5" />
          Redraw
        </button>
      </div>
      <div ref={containerRef} className="min-h-0 flex-1 overflow-hidden p-2" />
    </div>
  )
}
