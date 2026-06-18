// Truthful per-tab tmux signal (Option A). A pane records `tmuxByPaneKey[paneKey]
// = true` ONLY when it bound to a real server tmux session (`tmux_target`
// non-null) — never for a local PTY. A tab is tmux-backed iff ANY of its panes
// is. paneKey is `${tabId}:${leafUUID}` (see shared/stable-pane-id.ts), so a
// tab's panes are exactly the keys with the `${tabId}:` prefix.

/** Does this tab have at least one pane running in a real tmux session? */
export function isTabTmuxBacked(
  tmuxByPaneKey: Record<string, true>,
  tabId: string
): boolean {
  const prefix = `${tabId}:`
  for (const paneKey in tmuxByPaneKey) {
    if (paneKey.startsWith(prefix)) {
      return true
    }
  }
  return false
}
