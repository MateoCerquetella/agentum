// Build the shell command to attach to the tmux session backing an agent.
//
// Every agentum agent runs as a detached tmux session on its host (the local
// tmux server for local projects, the remote host's tmux server for SSH
// projects), so it survives closing the desktop. This composes the exact
// `tmux attach` command the user can run to drop into that live session — the
// sidebar's per-session "copy attach command" action.
import type { Session } from '@/runtime/agentum-server-client'
import type { ServerHost } from '@/runtime/server-host-client'

/**
 * The attach command for `session`, or `null` when it isn't attachable (no
 * running tmux target). A local session attaches directly; an SSH session wraps
 * `tmux attach` in `ssh -t` using the host's coords (`-p` omitted for port 22).
 *
 * `host` is the resolved server host (matched by `session.host_id`); when it's
 * missing for an ssh session we fall back to the bare local form rather than
 * emit a broken `ssh` line.
 */
export function buildTmuxAttachCommand(session: Session, host: ServerHost | undefined): string | null {
  const target = session.tmux_target?.trim()
  if (!target) {
    return null
  }
  const local = `tmux attach -t ${target}`

  const isSsh = (host?.kind ?? session.host_kind) === 'ssh'
  if (!isSsh) {
    return local
  }
  // SSH needs coords; without them we can't form a correct remote command.
  if (!host?.hostname || !host.user) {
    return local
  }
  const portFlag = host.port && host.port !== 22 ? ` -p ${host.port}` : ''
  return `ssh ${host.user}@${host.hostname}${portFlag} -t tmux attach -t ${target}`
}
