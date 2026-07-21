// Per-conversation repo-context status for Chat (spec 009 #361). The server
// leads every workspace-backed stream with a `context` event; `missing` means
// the pinned/selected project could not be read and the UI must say so — the
// alternative is the model apologizing turn after turn for a wiring bug.
// Pure module (no React, no runtime imports) so the reducer and the banner
// copy are plain vitest targets.

/** Conversation ids whose last stream reported missing repo context. */
export type ContextMissingMap = Readonly<Record<string, true>>

/** Fold one `context` event into the map, immutably. `ok` clears the flag
 *  (a later turn may ground after a transient SSH failure); `missing` sets it.
 *  Returns the SAME reference when nothing changes so store subscribers don't
 *  re-render. */
export function applyContextDelta(
  map: ContextMissingMap,
  convoId: string,
  state: 'ok' | 'missing'
): ContextMissingMap {
  if (state === 'missing') {
    if (map[convoId]) return map
    return { ...map, [convoId]: true }
  }
  if (!map[convoId]) return map
  const next: Record<string, true> = { ...map }
  delete next[convoId]
  return next
}

/** Clear a conversation's flag (used on a fresh send — same lifecycle as the
 *  per-conversation stream error). */
export function clearContextMissing(map: ContextMissingMap, convoId: string): ContextMissingMap {
  return applyContextDelta(map, convoId, 'ok')
}

/** The banner copy. Names the project when known so the user sees WHICH repo
 *  went unread, and says what the model is left with. */
export function contextWarningText(repoName: string | null): string {
  const subject = repoName ? `agentum couldn't read ${repoName}'s files` : "agentum couldn't read this project's files"
  return `${subject} for this chat — answers won't be grounded in the repo. Check that the project's path and host are reachable (see the server log: "chat repo-context gather").`
}
