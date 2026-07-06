// The Wiki probe plan (spec 009 AC-4): which repos a WikiPage mount (or a
// regeneration transition) may issue `GET /api/wiki` for — exactly the pinned
// repo, never any other. Deliberately trivial: it exists to make the
// one-repo-only contract explicit and unit-testable. The every-repo sweep this
// replaces probed all of `s.repos` on mount, shelling `git remote get-url` in
// each — the macOS TCC-prompt-storm trigger this spec kills. F3 absorbs this
// module into the `wiki-view-state.ts` reducer.

/** The repos a WikiPage mount probes: the pinned repo, nothing else. */
export function wikiProbePlan(pinnedRepoId: string): string[] {
  return [pinnedRepoId]
}
