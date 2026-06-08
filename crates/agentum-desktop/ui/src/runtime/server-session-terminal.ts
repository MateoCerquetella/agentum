// Binds a server session's terminal stream (a tmux pane over WS) to an xterm
// Terminal — the Option A primitive for rendering server-backed sessions in the
// desktop, the same panes the TUI drives. This is how SSH/remote sessions will
// survive disconnection: the pane lives in tmux on the server, not in a local
// PTY owned by the desktop process.
import type { Terminal } from '@xterm/xterm'
import { openSessionStream, type SessionStream } from './agentum-server-client'
import { extractAllOscTitles } from '../../../shared/agent-detection'

export type ServerSessionTerminalBinding = {
  /** Tear down the WS stream and detach the xterm listeners. */
  dispose: () => void
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
  /** Host bucket this session's WS throughput counts toward in the status-bar
   *  I/O meter (`'local'` or `'ssh:<connectionId>'`). Omitted → local host. */
  hostKey?: string
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

  const stream: SessionStream = await openSessionStream(
    sessionId,
    { cols: term.cols, rows: term.rows },
    {
      onData: (bytes) => {
        term.write(bytes)
        scanForTitles(bytes)
      },
      onClose: () => term.write('\r\n\x1b[2m[agentum: session stream closed]\x1b[0m\r\n')
    },
    opts?.hostKey
  )

  const dataSub = term.onData((data) => stream.send(data))
  const resizeSub = term.onResize(({ cols, rows }) => stream.resize(cols, rows))

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
      dataSub.dispose()
      resizeSub.dispose()
      stream.close()
    }
  }
}
