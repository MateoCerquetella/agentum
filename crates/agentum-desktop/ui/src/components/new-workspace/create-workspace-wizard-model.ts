// Pure, React/DOM-free logic behind `CreateWorkspaceWizard`. The UI package
// ships no jsdom, so the wizard's gradeable behaviors — step gating, the header
// recap, the agent-pill fallback, and the footer copy — live here where they
// can be unit-tested without mounting the component (mirrors the goal step's
// `workspace-goal-step.ts`). The component owns only local state + JSX.
import { filterEnabledTuiAgents, isTuiAgentEnabled } from '../../../../shared/tui-agent-selection'
import type { TuiAgent } from '../../../../shared/types'

export type WizardStep = 1 | 2 | 3

export const WIZARD_STEP_LABELS = ['Host', 'Repo & worktree', 'Agent & tracker'] as const

// Fallback agent pills for when detection hasn't produced a set yet (or found
// nothing installed) — kept small and catalog-ordered so the picker is never
// empty while `ensureDetectedAgents` is still in flight.
export const WIZARD_FALLBACK_AGENT_IDS: TuiAgent[] = ['claude', 'codex', 'gemini']

/** Step 2 (repo) can advance only once a repo is chosen and it doesn't still
 *  need an SSH connection — otherwise the create would fail at the gate. */
export function canLeaveRepoStep(input: {
  repoId: string
  requiresConnection: boolean
}): boolean {
  return Boolean(input.repoId) && !input.requiresConnection
}

/**
 * The agent pills to show. Prefer the enabled subset of the detected set; fall
 * back to the enabled catalog defaults so the picker is never empty (detection
 * is best-effort and can still be in flight or empty on a fresh host).
 */
export function resolveWizardAgentOptions(input: {
  detectedAgentIds: Iterable<TuiAgent> | null
  disabledTuiAgents?: Iterable<unknown> | null
  fallback?: TuiAgent[]
}): TuiAgent[] {
  const fallback = input.fallback ?? WIZARD_FALLBACK_AGENT_IDS
  if (input.detectedAgentIds) {
    const detected = [...input.detectedAgentIds].filter((id) =>
      isTuiAgentEnabled(id, input.disabledTuiAgents)
    )
    if (detected.length > 0) {
      return detected
    }
  }
  return filterEnabledTuiAgents(fallback, input.disabledTuiAgents)
}

/**
 * The truncatable header recap: the host, then repo·worktree once past step 1,
 * then the agent once past step 2 — mirrors what's been decided so far. Skips
 * absent pieces (blank worktree name, no agent) so it never shows dangling
 * separators.
 */
export function buildWizardRecap(input: {
  step: WizardStep
  hostLabel: string
  repoDisplayName?: string | null
  worktreeName?: string | null
  agent?: TuiAgent | null
}): string {
  const parts: string[] = [input.hostLabel]
  if (input.step > 1 && input.repoDisplayName) {
    const name = input.worktreeName?.trim()
    parts.push(name ? `${input.repoDisplayName} · ${name}` : input.repoDisplayName)
  }
  if (input.step > 2 && input.agent) {
    parts.push(input.agent)
  }
  return parts.join('  ·  ')
}

/** Primary button label: "Create workspace" on the last step, else "Continue". */
export function wizardPrimaryLabel(step: WizardStep): string {
  return step === 3 ? 'Create workspace' : 'Continue'
}

/** The label shown on the base-branch combobox trigger: the chosen ref, else
 *  the repo's resolved default ref, else a generic "default branch" hint. */
export function wizardBaseBranchTriggerLabel(
  baseBranch: string | undefined,
  defaultRef: string | null | undefined
): string {
  const chosen = baseBranch?.trim()
  if (chosen) return chosen
  const fallback = defaultRef?.trim()
  return fallback || 'default branch'
}

// ---------- Tracker (per-repo, honest) ----------

/**
 * The wizard's Tracker card is derived from the *selected repo's own remote*,
 * never a hardcoded string. Three honest states:
 * - `detected`: the remote parsed into a host + owner/repo slug.
 * - `disconnected`: the repo still needs a connection, so its remote isn't
 *   known yet — say so rather than claim a tracker.
 * - `none`: no git remote (or not a git repo / unparseable) — link one later.
 */
export type WizardTracker =
  | { kind: 'detected'; provider: TrackerProvider; label: string; host: string; slug: string }
  | { kind: 'disconnected' }
  | { kind: 'none' }

export type TrackerProvider = 'github' | 'gitlab' | 'other'

/** Human label for the provider chip: the brand for GitHub/GitLab, else the
 *  bare host (so a self-hosted remote still reads honestly, per-repo). */
function providerLabel(provider: TrackerProvider, host: string): string {
  if (provider === 'github') return 'GitHub'
  if (provider === 'gitlab') return 'GitLab'
  return host
}

/**
 * Parse a git remote URL into `{ host, slug, provider }`. Handles both the
 * scp-like SSH form (`git@github.com:owner/repo.git`) and URL forms
 * (`https://…`, `ssh://…`). Returns null for anything without a host + a
 * `owner/repo`-shaped path so callers can fall through to the honest "no
 * tracker" state instead of rendering a bogus slug.
 */
export function parseRemoteSlug(
  remoteUrl: string | null | undefined
): { host: string; slug: string; provider: TrackerProvider } | null {
  const raw = remoteUrl?.trim()
  if (!raw) return null

  let host: string
  let path: string
  // scp-like SSH: user@host:path — has a colon before any slash and no scheme.
  const scp = /^[^/@]+@([^:/]+):(.+)$/.exec(raw)
  if (scp && !raw.includes('://')) {
    host = scp[1]
    path = scp[2]
  } else {
    try {
      const url = new URL(raw)
      host = url.hostname
      path = url.pathname
    } catch {
      return null
    }
  }

  const slug = path
    .replace(/^\/+/, '')
    .replace(/\.git$/i, '')
    .replace(/\/+$/, '')
  if (!host || !slug || !slug.includes('/')) return null

  const lowerHost = host.toLowerCase()
  const provider: TrackerProvider =
    lowerHost === 'github.com' || lowerHost.endsWith('.github.com')
      ? 'github'
      : lowerHost === 'gitlab.com' || lowerHost.includes('gitlab')
        ? 'gitlab'
        : 'other'
  return { host, slug, provider }
}

/** Derive the tracker card state for the selected repo. Fails closed: an
 *  unparseable/absent remote never yields a fabricated "detected". */
export function deriveWizardTracker(input: {
  remoteUrl: string | null | undefined
  requiresConnection: boolean
  isGit: boolean
}): WizardTracker {
  if (!input.isGit) return { kind: 'none' }
  const parsed = parseRemoteSlug(input.remoteUrl)
  if (parsed) {
    return {
      kind: 'detected',
      provider: parsed.provider,
      label: providerLabel(parsed.provider, parsed.host),
      host: parsed.host,
      slug: parsed.slug
    }
  }
  // No remote we can read. If the repo still needs a connection, the remote
  // simply isn't known yet — that's "not connected", not "no tracker".
  if (input.requiresConnection) return { kind: 'disconnected' }
  return { kind: 'none' }
}
