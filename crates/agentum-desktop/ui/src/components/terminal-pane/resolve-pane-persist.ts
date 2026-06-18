// Spec 005-C: decide whether a single pane runs in a persistent tmux session
// (the server-backed path) or as an ephemeral local PTY.
//
// The toggle is per-tab: "Run in tmux (persist)" is stamped onto the tab at
// creation (default on) and remembered across the pane's life (it is persisted
// in the workspace session). When a tab carries no explicit choice — older
// persisted tabs, or panes created outside the New Terminal / New Agent flows —
// we fall back to the global default (`shouldUseServerTerminals`, also on), so
// existing behaviour is unchanged.
//
// Keeping this a pure function (no store / localStorage reads) makes the
// decision trivially testable and keeps the lifecycle branch a one-liner.
export type PanePersistDecisionInput = {
  /** The tab's explicit "Run in tmux (persist)" choice, if it made one. */
  tabPersistTmux: boolean | undefined | null
  /** The global default to use when the tab has no explicit choice. Defaults
   *  to `shouldUseServerTerminals()` at the call site. */
  globalDefault: boolean
  /** True when this tab launched an agent (claude/codex/cursor/…). Agents are
   *  forced onto the server/tmux path regardless of the persist toggle. */
  isAgentTab?: boolean
}

/**
 * Returns true when the pane should use the server/tmux (persistent) path,
 * false when it should use the ephemeral local PTY path.
 *
 * Agent tabs ALWAYS use the server path: the server launches the agent through
 * its tool adapter as one clean process, whereas the local PTY path spawns a
 * shell and injects the launch command — which for an agent can land the
 * command itself in the agent's composer ("claude" typed into Claude) and
 * double-launch. The local PTY path is a half-ported stub; agents need the
 * proven one. For plain terminals the tab's explicit choice wins; otherwise the
 * global default decides.
 */
export function resolvePaneUsesServerSession(input: PanePersistDecisionInput): boolean {
  if (input.isAgentTab) {
    return true
  }
  if (input.tabPersistTmux === true) {
    return true
  }
  if (input.tabPersistTmux === false) {
    return false
  }
  return input.globalDefault
}

// ─── "Run in tmux (persist)" default ────────────────────────────────────────
//
// Lives here (a leaf module with no store imports) rather than alongside
// `shouldUseServerTerminals` in server-pane-connection.ts, which imports the
// app store. The store slice (`createTab`) reads this default, and routing it
// through server-pane-connection would create a cycle
// (terminals → server-pane-connection → store → terminals) that breaks slice
// initialization order.

/** localStorage key for the "Run in tmux (persist)" default applied to the next
 *  New Terminal / New Agent. Distinct from `agentum.serverTerminals` (the global
 *  kill-switch) — this only seeds the per-tab choice at creation. */
const PERSIST_TMUX_DEFAULT_KEY = 'agentum.persistTmuxDefault'

/**
 * Spec 005-C: the "Run in tmux (persist)" default the New Terminal / New Agent
 * flows stamp onto a freshly created tab. Defaults to ON (persist), matching the
 * product vision; off is the explicit, remembered opt-out. Persisted so the
 * user's last choice carries to the next new pane.
 */
export function getPersistTmuxDefault(): boolean {
  try {
    return globalThis.localStorage?.getItem(PERSIST_TMUX_DEFAULT_KEY) !== '0'
  } catch {
    return true
  }
}

/** Remember the user's "Run in tmux (persist)" choice for the next new pane. */
export function setPersistTmuxDefault(persist: boolean): void {
  try {
    globalThis.localStorage?.setItem(PERSIST_TMUX_DEFAULT_KEY, persist ? '1' : '0')
  } catch {
    /* localStorage unavailable (e.g. SSR/tests) — the in-memory default is on */
  }
}
