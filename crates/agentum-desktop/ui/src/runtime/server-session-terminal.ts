// Binds a server session's terminal stream (a tmux pane over WS) to an xterm
// Terminal — the Option A primitive for rendering server-backed sessions in the
// desktop, the same panes the TUI drives. This is how SSH/remote sessions will
// survive disconnection: the pane lives in tmux on the server, not in a local
// PTY owned by the desktop process.
import type { Terminal } from '@xterm/xterm'
import { openSessionStream, type SessionStream } from './agentum-server-client'
import {
  markHostConnectedFromHostKey,
  markHostReconnectingFromHostKey
} from './server-host-client'
import { extractAllOscTitles } from '@/shared/agent-detection'
import {
  writeTerminalOutput,
  flushTerminalOutput,
  discardTerminalOutput
} from '@/lib/pane-manager/pane-terminal-output-scheduler'

export type ServerSessionTerminalBinding = {
  /** Tear down the WS stream and detach the xterm listeners. */
  dispose: () => void
  /** Force the agent to fully repaint the pane (SIGWINCH nudge + fresh
   *  snapshot). Backs the manual "force redraw" shortcut; the binding also
   *  fires this automatically on reconnect. */
  forceRedraw: () => void
  /** Send raw bytes into the session's input stream — the SAME channel
   *  `term.onData` uses. Backs intercepted keyboard chords (word-nav/erase,
   *  line-nav) that bypass xterm's own key handling and therefore never reach
   *  `onData`; without this they were silently dropped for server sessions. */
  sendInput: (data: string) => void
}

export type BindServerSessionTerminalOptions = {
  /** A command to run once the pane is live (e.g. `claude`) — the agent launch
   *  the desktop would otherwise type into a local shell. Sent after the stream
   *  connects so the agent actually attaches instead of leaving a bare shell. */
  startupCommand?: string
  /** Called with each new OSC title seen in the pane stream. Why: server
   *  sessions write pane bytes straight to xterm, so — unlike the local PTY
   *  path — the agent-status pipeline never saw the agent's title. Without this
   *  the sidebar working/idle/needs-attention state stays blank for every
   *  tmux-backed agent. The bytes carry the title via the pipe-pane tail; we
   *  extract it here and let the caller route it into runtimePaneTitlesByTabId. */
  onTitle?: (title: string) => void
  /** Called on every server → client byte chunk. Why: agents whose OSC title
   *  carries no working/idle signal (OpenCode, Codex) leave the title path
   *  blind, so the sidebar would show them idle while they stream output. Byte
   *  arrival is the same "pane is redrawing" signal the daemon watchdog polls
   *  for; the caller debounces it into a working/idle state. */
  onActivity?: () => void
  /** Host bucket this session's WS throughput counts toward in the status-bar
   *  I/O meter (`'local'` or `'ssh:<connectionId>'`). Omitted → local host. */
  hostKey?: string
  /** Whether this pane is currently the visible/focused one, read live at each
   *  write. Why: server-session output is routed through the shared terminal
   *  output scheduler (same as the native PTY path) so BACKGROUND panes are
   *  throttled instead of every agent writing straight to xterm on the main
   *  thread — the many-agents lag. A visible pane returns true → synchronous
   *  write (unchanged responsiveness); hidden → false → batched/throttled.
   *  Defaults to foreground (true) when omitted so callers that don't wire it
   *  keep the old immediate-write behavior. */
  isForeground?: () => boolean
}

/**
 * Connect `term` to session `sessionId`'s tmux pane:
 * - pane bytes (server → client) are written into the terminal,
 * - keystrokes (`term.onData`) are forwarded as binary WS frames,
 * - resizes (`term.onResize`) are forwarded as JSON resize frames.
 *
 * The caller owns the Terminal's lifecycle; `dispose()` only tears down what
 * this binding created.
 */
export async function bindServerSessionTerminal(
  sessionId: string,
  term: Terminal,
  opts?: BindServerSessionTerminalOptions
): Promise<ServerSessionTerminalBinding> {
  let disposed = false
  let startupTimer: ReturnType<typeof setTimeout> | null = null

  // OSC title extraction state. The pane stream is raw tmux bytes; agent CLIs
  // announce working/idle/permission in their OSC title (`\x1b]0;…\x07`). We
  // accumulate just enough to span a title sequence split across WS frames,
  // emit each distinct title once, then trim to the unterminated tail.
  const titleDecoder = new TextDecoder()
  let titleScanBuf = ''
  let lastForwardedTitle: string | null = null
  // Why: the scheduler's writeTerminalOutput takes a string, but pane bytes
  // arrive as Uint8Array chunks that can split a multi-byte UTF-8 codepoint
  // across WS frames. A dedicated STREAMING decoder ({stream:true}) reproduces
  // xterm's own stateful decode so a split codepoint isn't corrupted into
  // U+FFFD. Must be separate from titleDecoder — sharing state would
  // double-consume the same bytes across the two decode paths.
  const outputDecoder = new TextDecoder()
  const foreground = (): boolean => opts?.isForeground?.() ?? true
  const scanForTitles = (bytes: Uint8Array): void => {
    if (!opts?.onTitle) {
      return
    }
    titleScanBuf += titleDecoder.decode(bytes, { stream: true })
    for (const title of extractAllOscTitles(titleScanBuf)) {
      if (title !== lastForwardedTitle) {
        lastForwardedTitle = title
        opts.onTitle(title)
      }
    }
    // Keep only bytes after the last OSC terminator (a possible partial title);
    // bound memory so a title-less stream can't grow the buffer unboundedly.
    const lastTerminator = Math.max(titleScanBuf.lastIndexOf('\x07'), titleScanBuf.lastIndexOf('\x1b\\'))
    if (lastTerminator >= 0) {
      titleScanBuf = titleScanBuf.slice(lastTerminator + 1)
    }
    if (titleScanBuf.length > 8192) {
      titleScanBuf = titleScanBuf.slice(-8192)
    }
  }

  // Forward handle so the reconnect callback (defined inline below, before the
  // handle exists) can ask for a redraw heal once the stream is live.
  let sessionStream: SessionStream | null = null

  const stream: SessionStream = await openSessionStream(
    sessionId,
    { cols: term.cols, rows: term.rows },
    {
      onData: (bytes) => {
        // Route pane output through the shared scheduler (like the native PTY
        // path) so background panes are throttled instead of every agent
        // writing straight to xterm. Title scan + activity stay synchronous on
        // the raw bytes so sidebar state never lags the throttled render.
        writeTerminalOutput(term, outputDecoder.decode(bytes, { stream: true }), {
          foreground: foreground(),
          // Unlike the native PTY path, a server session has no main-buffer
          // snapshot to restore from, so if a hidden agent floods past the
          // scheduler's 8 MB background cap the dropped bytes are gone. Force a
          // fresh server snapshot to re-sync the pane instead of leaving a hole.
          onBackgroundBacklogDropped: () => {
            if (!disposed) {
              sessionStream?.requestRepaint()
            }
          }
        })
        scanForTitles(bytes)
        opts?.onActivity?.()
      },
      // Permanent close: the session is gone or our token was rejected. A
      // transient drop reconnects silently (onReconnecting) instead of this.
      // Route through the scheduler so the banner can't paint ahead of pane
      // output still queued for a background pane (writeTerminalOutput flushes
      // the queue before a foreground write, and enqueues in order otherwise).
      onClose: () =>
        writeTerminalOutput(term, '\r\n\x1b[2m[agentum: session stream closed]\x1b[0m\r\n', {
          foreground: foreground()
        }),
      // First drop of a reconnect cycle: show one dim hint. A successful
      // reconnect repaints the pane (the server replays a snapshot) and wipes
      // this line; printing only on attempt 1 keeps a long outage from spamming
      // the scrollback. `attempt` resets after a connection holds, so a later
      // independent drop hints again.
      onReconnecting: (attempt) => {
        if (attempt === 1) {
          writeTerminalOutput(
            term,
            '\r\n\x1b[2m[agentum: connection lost — reconnecting…]\x1b[0m\r\n',
            { foreground: foreground() }
          )
          // Reflect the outage in the SSH badge for this host (and arm the next
          // recovery's generation bump). Keyed off hostKey; no-op for local.
          void markHostReconnectingFromHostKey(opts?.hostKey)
        }
      },
      // Recovered: the re-attach proves the host is reachable, so re-mark it
      // connected. That repaints the SSH badges and bumps sshConnectedGeneration,
      // re-triggering the file explorer's retry — fixing the bug where the
      // terminal reconnected but the sidebar/tree stayed stuck on the outage.
      onReconnected: () => {
        void markHostConnectedFromHostKey(opts?.hostKey)
        // A reconnect is the suspend/resume path: while we were gone, an OS
        // `wall` broadcast (e.g. systemd's "system will suspend now!") may
        // have been written straight into the pane grid, and the resume
        // replay just re-feeds it — the agent won't overpaint cells it didn't
        // draw. Force a full repaint (SIGWINCH nudge) so the garbage heals
        // instead of persisting until the user types. No-op on old daemons.
        sessionStream?.requestRedraw()
      }
    },
    opts?.hostKey
  )
  sessionStream = stream

  const dataSub = term.onData((data) => stream.send(data))
  const resizeSub = term.onResize(({ cols, rows }) => stream.resize(cols, rows))

  // ── Blank-pane self-heal ────────────────────────────────────────────────
  // A remote IDLE pane's only paint source is the one connect snapshot — no
  // live bytes follow it. If that snapshot never lands on screen (lost when the
  // xterm reflows during a multi-pane restore on reopen, or the server returned
  // an EMPTY snapshot under SSH ControlMaster channel pressure), the pane sits
  // BLANK forever. We can't cheaply tell which boundary dropped it, so we watch
  // the OBSERVABLE symptom: if the terminal still shows nothing a few seconds
  // after connecting, force a fresh re-snapshot. Bounded so a genuinely-empty
  // pane can't loop; a pane that actually painted is non-blank and never fires.
  const PAINT_GRACE_MS = 6000
  const MAX_REPAINTS = 2
  let repaintAttempts = 0
  let paintWatchdog: ReturnType<typeof setTimeout> | null = null

  const paneLooksBlank = (): boolean => {
    try {
      const buf = term.buffer.active
      for (let i = 0; i < buf.length; i++) {
        if ((buf.getLine(i)?.translateToString(true).trim().length ?? 0) > 0) {
          return false
        }
      }
      return true
    } catch {
      // Can't introspect the buffer → assume it painted; never self-heal blind.
      return false
    }
  }

  const armPaintWatchdog = (): void => {
    if (paintWatchdog) {
      clearTimeout(paintWatchdog)
      paintWatchdog = null
    }
    if (disposed || repaintAttempts >= MAX_REPAINTS) {
      return
    }
    paintWatchdog = setTimeout(() => {
      paintWatchdog = null
      if (disposed) {
        return // torn down — nothing to heal
      }
      // Why: output is now scheduled, so a background pane's connect snapshot
      // can be queued but not yet drained to xterm. Flush it first so the blank
      // check reads the real painted state instead of firing a spurious repaint.
      flushTerminalOutput(term)
      if (!paneLooksBlank()) {
        return // painted — nothing to heal
      }
      repaintAttempts += 1
      stream.requestRepaint()
      armPaintWatchdog() // give the fresh snapshot its own grace window
    }, PAINT_GRACE_MS)
  }
  armPaintWatchdog()

  // Launch the agent once the shell has had a moment to come up. tmux buffers
  // input, so an early send is harmless; the short delay just avoids racing the
  // prompt. Trailing CR submits the command.
  const startup = opts?.startupCommand?.trim()
  if (startup) {
    startupTimer = setTimeout(() => {
      if (!disposed) {
        stream.send(`${startup}\r`)
      }
    }, 500)
  }

  return {
    dispose: () => {
      disposed = true
      if (startupTimer) {
        clearTimeout(startupTimer)
      }
      if (paintWatchdog) {
        clearTimeout(paintWatchdog)
        paintWatchdog = null
      }
      // Purge any queued chunks + cancel the foreground render-settle so a late
      // background drain can't write into the torn-down xterm (mirrors the
      // native PTY path's discard on teardown).
      discardTerminalOutput(term)
      dataSub.dispose()
      resizeSub.dispose()
      stream.close()
    },
    forceRedraw: () => {
      if (!disposed) {
        stream.requestRedraw()
      }
    },
    sendInput: (data: string) => {
      if (!disposed) {
        stream.send(data)
      }
    }
  }
}
