// Binds a server session's terminal stream (a tmux pane over WS) to an xterm
// Terminal — the Option A primitive for rendering server-backed sessions in the
// desktop, the same panes the TUI drives. This is how SSH/remote sessions will
// survive disconnection: the pane lives in tmux on the server, not in a local
// PTY owned by the desktop process.
import type { Terminal } from '@xterm/xterm'
import { openSessionStream, type SessionStream } from './agentum-server-client'

export type ServerSessionTerminalBinding = {
  /** Tear down the WS stream and detach the xterm listeners. */
  dispose: () => void
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
  term: Terminal
): Promise<ServerSessionTerminalBinding> {
  const stream: SessionStream = await openSessionStream(
    sessionId,
    { cols: term.cols, rows: term.rows },
    {
      onData: (bytes) => term.write(bytes),
      onClose: () => term.write('\r\n\x1b[2m[agentum: session stream closed]\x1b[0m\r\n')
    }
  )

  const dataSub = term.onData((data) => stream.send(data))
  const resizeSub = term.onResize(({ cols, rows }) => stream.resize(cols, rows))

  return {
    dispose: () => {
      dataSub.dispose()
      resizeSub.dispose()
      stream.close()
    }
  }
}
