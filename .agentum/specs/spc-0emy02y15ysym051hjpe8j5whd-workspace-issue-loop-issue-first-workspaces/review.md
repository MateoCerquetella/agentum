# Spec 004 — Reviewer Sign-off

- **Date:** 2026-07-01 (autonomous /sdd-loop iteration 6)
- **Verdict:** **SIGN-OFF — SHIP-READY.** No Blockers. Release stays
  human-gated; AC 1 chip render, AC 6 toggle flow, and live label movement
  remain staging browser-QA items as contracted.

## Focus-item verification (all pass)

1. **One-logical-line seam touches** — `transition_tracker` threads
   `feature.tracker_url.as_deref()` (rustfmt reflow only); board_goals
   initial-Todo passes `url.as_deref()`; transition points + autonomy
   mechanics untouched.
2. **`Ok(Skipped)`-never-`Err`** — the github arm has no `?` and constructs no
   `Err`; ensure-create failures `let _ =`; edit failure → `Skipped(reason)`;
   no close/state mutation (D1 holds).
3. **C3 wire keys** — alias on `CreateBody.linked_pr` only; registry struct
   alias-free with the `!obj.contains_key("linkedPR")` regression pin;
   detected scan emits `linkedPR`; `canonical_meta_key` in `update_meta`.
4. **Bare `- [ ]` lines** — body appended verbatim (control-stripped, never
   prefixed); fallback AC decided by running the REAL
   `derive_backlog_from_spec`, so the round-trip survives 64 KiB truncation.
5. **Auth** — all new routers merge before `require_token` (lib.rs:305-313);
   `is_public` carries only the six pre-existing paths.
6. **UI consistency** — Card affordance/mini-form/toggle match composer
   patterns (Button/label/role="alert"/disabled-spinner); reentrancy guard;
   zero-state-change failure; `maybeScaffoldSpecFromIssue` re-derives its gate
   at submit, non-fatal, in both submit paths.

## Findings

**Blocker:** none.

**Should-fix (follow-ups, non-blocking):**
1. `useComposerState.ts:1448` — `as unknown as GitHubWorkItem` double cast:
   safe today (consumers read only type/number/title/url) but silences the
   compiler if the type grows; narrow the parameter instead. Role: developer,
   any later pass.
2. **GHES follow-up (file an issue):** `github_slug_and_number_from_issue_url`
   hard-pins `https://github.com/` — a GHES issue creates fine via `gh` but
   every transition degrades to `Skipped`. In-scope per the GitHub-only
   decision; name it so it isn't rediscovered as a bug.

**Nits:**
1. board_goals initial-Todo `Ok(_) => {}` swallows `Skipped(reason)` silently
   (drive-loop transitions log theirs); one `tracing::debug!` would make
   "labels never appeared at plan time" diagnosable.
2. spec.md Status header read `PM` — flipped to Done at sign-off (this pass).
3. `scaffoldSpec` toggle survives unlink/relink of the work item; harmless
   (gate re-derived at submit) but reset-on-unlink would match D5's spirit.

**Tester Info findings 2–4:** correctly weighted; none rise above Info.

## Overall assessment (verbatim)

Disciplined work. The change surface matches the architecture blueprint
symbol-for-symbol (C1–C5 honored); the invariants hold — task_sink stays
route-free, the harness transforms stay pure beside their kin, one-launch-path
and push-not-poll untouched, MCP behavior pinned by the delegation test. The
test suite is unusually communicative: the C4 exactly-one-canonical-label
invariant, the registry no-alias wipe guard, and the traversal-proof spec id
are each locked by a self-describing test. Comment discipline is exemplary,
and the previously-lying comment at useComposerState.ts:2209 now tells the
truth because F2 made it true. tasks.md is accurate point-by-point.

## Ship path (human-gated from here)

`/ship`-style flow: labeled GitHub issue + PR `fix-wiki` → `develop` with
`Closes #<issue>`; staging browser QA (chip, toggle, live `status/*` flip
ending OPEN with exactly `status/done`); then promote + tag per branch flow.
