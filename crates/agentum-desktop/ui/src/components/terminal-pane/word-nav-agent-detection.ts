import { isExplicitAgentStatusFresh, getAgentLabel } from '@/lib/agent-status'
import { AGENT_STATUS_STALE_AFTER_MS, type AgentStatusEntry } from '@/shared/agent-status-types'

/**
 * Decide whether the active terminal pane is owned by an interactive agent CLI
 * for the purpose of Option/Ctrl+Arrow WORD navigation. The byte encoding
 * differs by target: an agent CLI (Claude Code, Codex, …) reads the standard
 * cursor CSI (\e[1;3D / \e[1;3C) for word motion, while a bare shell needs
 * readline's \eb / \ef — there is no single sequence both honor, so the chord
 * has to know which one owns the pane.
 *
 * Two independent signals, OR'd, because each alone leaves a real gap:
 *  - Explicit hook status (agentStatusByPaneKey) is authoritative but OPT-IN —
 *    a pane whose agent never POSTs status hooks has no entry at all, so
 *    relying on it alone left word-nav broken inside Claude Code / Codex: the
 *    chord fell back to \eb / \ef, which those agents ignore (the reported bug).
 *  - The live OSC title is hook-independent and pane-specific (keyed by
 *    pane.id). getAgentLabel is the SAME classifier the sidebar uses to derive
 *    agent rows from titles, so detection stays consistent across the app.
 *
 * The title path's only false-positive is a bare shell whose OSC title happens
 * to contain an agent-name substring (e.g. a shell sitting in ~/codex/…). That
 * is rare, matches the sidebar's already-accepted detection, and degrades only
 * to "word-nav emits the agent CSI in that shell" — no destructive effect, and
 * it never overrides the authoritative hook signal.
 */
export function paneRunsAgentForWordNav(
  entry: Pick<AgentStatusEntry, 'updatedAt'> | undefined,
  paneTitle: string | undefined,
  now: number
): boolean {
  if (entry != null && isExplicitAgentStatusFresh(entry, now, AGENT_STATUS_STALE_AFTER_MS)) {
    return true
  }
  return paneTitle != null && getAgentLabel(paneTitle) !== null
}
