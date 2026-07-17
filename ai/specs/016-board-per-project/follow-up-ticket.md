# Ready-to-file follow-up ticket (S1 + S2 from review.md)

> The autonomous run could not create GitHub issues (external-write
> permission). File this with:
> `gh issue create --repo MateoCerquetella/agentum --label "type/fix,area/desktop,priority/p3" --title "<title below>" --body-file <this section>`

**Title:** Board per-project follow-up: first-frame legacy flash on first hub-Tasks visit + ghost settings-search entry

## Summary

Two Should-fix findings from the spec 016 reviewer sign-off
(`ai/specs/016-board-per-project/review.md` §3). Neither blocks the #360
release.

## S1 — first-frame legacy render + spurious legacy fetches on the FIRST hub-Tasks visit of a session

A missing `projectBindingByRepo` entry maps to
`BINDING_ABSENT = {status:'loaded', binding:null}` (`ProjectViewWrapper.tsx`,
`TaskPage.tsx`), which resolves past the `pending` state to the legacy/none
tier. React runs child effects before parent effects, so on the first
hub-Tasks visit the wrapper's auto-fetch + view-list effects fire for the
LEGACY project before `ProjectHubPage`'s binding effect writes `loading`.
Result: one paint of the wrong surface (or, with a warm `projectViewCache`
from a standalone visit, a one-frame flash of the wrong project's board)
plus wasted gh/RPC fetches. Converges on the next frame; no settings write.

**Proposed fix:** treat a missing store entry as `{status:'loading'}` when
`repoId != null` (embedded), or seed the `loading` entry synchronously in
`openProjectHub`.

**Repro (also the qa.sh watchpoint):** visit the standalone board (legacy
project cached) → open a bound repo's hub Tasks tab for the first time that
session → watch the first frame.

## S2 — ghost "Show Tasks Button" entry in settings search

`components/settings/appearance-search.ts:120-125` still advertises the
toggle deleted in spec 016 F3 (`4b98dd73`); searching settings for
"tasks"/"sidebar" surfaces an entry pointing at nothing. One-line deletion.

## Acceptance criteria

- [ ] First hub-Tasks visit of a session renders skeleton → bound board,
      with zero fetches for the legacy project (assert via network/RPC log).
- [ ] Settings search for "tasks" no longer returns the removed toggle.
- [ ] `bun run build` + spec-016 vitest suites stay green.

Found by the autonomous SDD run for #360.
