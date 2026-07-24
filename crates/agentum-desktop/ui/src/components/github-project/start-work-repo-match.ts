import type { Repo } from '../../../../shared/types'

// Why: post spec-015 F1 the registry can hold the same owner/repo slug on
// several hosts (same path, local + remote). The board's Start-work used to
// collapse `matches.length === 1 ? matches[0] : null`, which turns a dual
// entry into a false "Repository isn't added to Agentum" dialog. This pure
// classifier is the single decision point the board's start gestures share.
export type StartWorkRepoMatch =
  | { kind: 'none' }
  | { kind: 'direct'; repo: Repo }
  | { kind: 'choose'; repos: Repo[]; seedRepoId: string }

/** Classify a slug's registered matches for board Start-work. Exactly one →
 *  the direct path (AC 6, byte-equivalent friction). Multiple → the wizard
 *  hop; seed = the local copy when present, else the first match, so the
 *  composer opens on a host that actually holds this repo. */
export function classifyStartWorkRepoMatches(matches: Repo[]): StartWorkRepoMatch {
  if (matches.length === 0) {
    return { kind: 'none' }
  }
  if (matches.length === 1) {
    return { kind: 'direct', repo: matches[0] }
  }
  const seed = matches.find((repo) => repo.connectionId == null) ?? matches[0]
  return { kind: 'choose', repos: matches, seedRepoId: seed.id }
}
