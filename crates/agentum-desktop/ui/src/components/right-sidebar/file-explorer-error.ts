// Classifies file-explorer root-load failures so the tree can show a friendly
// "host unreachable" state for SSH transport failures while keeping genuine
// path/permission/auth errors diagnosable (those still render their message).

// Connection-level ssh stderr fragments, surfaced through the server's
// host-aware fs routes as `remote fs: ssh/tmux exited with status Some(255)
// (stderr: ssh: connect to host <ip> port 22: Operation timed out)`.
const SSH_CONNECT_FAILURE_SIGNATURES = [
  'connection timed out',
  'operation timed out',
  'connection refused',
  'no route to host',
  'network is unreachable',
  'could not resolve hostname',
  'name or service not known',
  'connection reset by peer',
  'connection closed by remote host'
]

// Signatures that already carry their ssh context, so they classify on their
// own: the ssh client's connect-failure prefix and the server-side ssh
// deadline (`host_runtime::HostRuntimeError::Timeout`).
const SELF_EVIDENT_SSH_FAILURE_SIGNATURES = ['ssh: connect to host', 'ssh command timed out']

/**
 * True when a file-explorer load error means the workspace's SSH host could
 * not be reached (down host, VPN off, DNS failure), as opposed to a real fs
 * error on a reachable host. Auth failures (e.g. "Permission denied
 * (publickey)") intentionally do NOT classify — their message is actionable.
 */
export function isHostUnreachableFsError(message: string): boolean {
  const normalized = message.toLowerCase()
  if (SELF_EVIDENT_SSH_FAILURE_SIGNATURES.some((sig) => normalized.includes(sig))) {
    return true
  }
  // Bare connection signatures only count in a remote-fs context; a local
  // error that happens to mention "connection refused" is not an SSH outage.
  return (
    normalized.includes('remote fs:') &&
    SSH_CONNECT_FAILURE_SIGNATURES.some((sig) => normalized.includes(sig))
  )
}
