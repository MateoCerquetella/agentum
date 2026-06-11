// "Add Account" for Claude is a capture of the *live* login (claude-swap
// model): the backend snapshots whatever credentials are currently on the
// machine. That means clicking Add while the live login is already saved can
// never produce a second account — the user must sign out, sign in with the
// other account, and capture again. These helpers decide which of those paths
// a click should take; the dialog state machine in AccountsPane drives them.

export type ClaudeLiveLogin = {
  hasCredentials: boolean
  email: string | null
}

export type ClaudeAddAccountDecision =
  /** Live login exists and isn't saved yet — capture it directly. */
  | { kind: 'capture' }
  /** Live login is already saved — offer the sign-out hand-off so the user
   *  can sign in with a different account. */
  | { kind: 'confirm-signout'; email: string }
  /** Nothing is signed in — wait for a fresh `claude` sign-in to capture. */
  | { kind: 'wait-for-login' }

export function decideClaudeAddAccount(
  live: ClaudeLiveLogin,
  managedEmails: string[]
): ClaudeAddAccountDecision {
  if (!live.hasCredentials) {
    return { kind: 'wait-for-login' }
  }
  const email = live.email
  if (email && managedEmails.some((managed) => managed.toLowerCase() === email.toLowerCase())) {
    return { kind: 'confirm-signout', email }
  }
  return { kind: 'capture' }
}

/** A fresh sign-in is capturable once both the credentials and the identity
 *  block exist — Claude Code writes them at slightly different moments, and
 *  capturing before the email lands would break the email-keyed dedupe. */
export function isClaudeLoginCaptureReady(live: ClaudeLiveLogin): boolean {
  return live.hasCredentials && Boolean(live.email)
}

// Strip ANSI/OSC escape sequences so a URL split across color codes still
// matches. Covers CSI (`\x1b[…`) and OSC (`\x1b]…`) runs.
// eslint-disable-next-line no-control-regex
const ANSI_ESCAPE = /\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g

const LOGIN_URL_HOST_HINTS = ['oauth', 'authorize', 'claude.ai', 'anthropic.com', 'console.anthropic']

/**
 * Pull the OAuth sign-in URL out of `claude auth login` output so it can be
 * surfaced as a clickable link. Returns the first https URL that points at a
 * Claude/Anthropic login host, or null if none has been printed yet (the
 * caller keeps accumulating output until one appears).
 */
export function extractClaudeLoginUrl(output: string): string | null {
  const clean = output.replace(ANSI_ESCAPE, '')
  const matches = clean.match(/https?:\/\/[^\s'"`<>()[\]]+/g)
  if (!matches) {
    return null
  }
  const hit = matches.find((url) => {
    const lower = url.toLowerCase()
    return LOGIN_URL_HOST_HINTS.some((hint) => lower.includes(hint))
  })
  // Trim trailing punctuation the terminal may have appended (".", ",", etc.).
  return hit ? hit.replace(/[.,;:]+$/, '') : null
}
