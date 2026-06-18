// Ephemeral hand-off of a typed prompt from the New Workspace composer to the
// "Start a session" picker (WorkspaceAgentLauncher).
//
// Why: creating a workspace no longer auto-launches an agent — it lands on the
// picker so the user chooses what to start. A prompt the user typed in the
// composer would otherwise be silently lost. We stash it by worktreeId here and
// the picker consumes it (read-once) when an agent is finally picked, delivering
// it as an editable draft. A plain module Map (not the store) is enough: the
// value is read exactly once at click time, never rendered reactively, and a
// stale entry for a discarded worktree is harmless.
const pendingByWorktree = new Map<string, string>()

/** Remember a typed prompt for `worktreeId`. No-op for whitespace-only input. */
export function stashPendingSessionPrompt(worktreeId: string, prompt: string): void {
  const trimmed = prompt.trim()
  if (trimmed) {
    pendingByWorktree.set(worktreeId, trimmed)
  } else {
    pendingByWorktree.delete(worktreeId)
  }
}

/** Read and clear the pending prompt for `worktreeId` (undefined if none). */
export function takePendingSessionPrompt(worktreeId: string): string | undefined {
  const prompt = pendingByWorktree.get(worktreeId)
  if (prompt !== undefined) {
    pendingByWorktree.delete(worktreeId)
  }
  return prompt
}
