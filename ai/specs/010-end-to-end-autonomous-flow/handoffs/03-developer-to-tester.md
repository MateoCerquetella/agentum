# Handoff 03 — Developer → Tester

- **Spec:** 010-end-to-end-autonomous-flow
- **Date:** 2026-07-06
- **From:** Developer (autonomous /sdd-loop; three gated slices)
- **To:** Tester
- **Artifacts:** commits **F1 `474cfd12`** (board bind) · **F2 `0b03eb9e`**
  (drive) · **F3 `26b1e022`** (provision); `tasks.md` (per-slice record +
  25 documented deviations); base = origin/develop v0.59.1 merged.

## Gate results as reported (tester re-runs ALL independently)

| Gate | Expected |
|---|---|
| `cargo test -p agentum-server --lib` | **616 passed / 0 failed / 5 ignored** (571 pre-spec baseline + 20 F1 + 13 F2 + 12 F3) |
| `cargo fmt --all --check` + `cargo clippy --workspace` | clean / 0 warnings (worktree needs the sherpa/onnx dylibs already copied into `target/release/`) |
| `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix crates/agentum-desktop/ui` | green (~1.5–3.5 min) |
| `npx vitest run src/lib/workspace-provision-step.test.ts src/lib/workspace-goal-step.test.ts src/lib/github-projects-binding.test.ts` (in `crates/agentum-desktop/ui`) | **37 / 0** |
| Baselines (judge DELTAS only) | full vitest ≈31 pre-existing failing files; bare `npx tsc --noEmit` = **1642** pre-existing errors; neither grew |

## AC → evidence map (spec.md ACs 1–11)

- **AC 1–3 (F1 bind):** binding persistence + constructor invariant
  (`github_projects.rs` tests: missing-phase file reads as no-binding);
  AC 2's four fixtures = `resolve_default_board…`, `resolve_custom_backlog_building_qa_shipped`,
  `resolve_no_rtt_column…`, `classify_scope_missing_names_gh_auth_refresh`
  (+ fake-gh discovery); AC 3 = `ProjectBindingEditor` (per-phase selects,
  FellBack hints, refusal → manual completion) mounted in Settings →
  Integrations → GitHub → "Projects v2 board".
- **AC 4 (F2 seam):** `board_write_with_fake_gh_cold_is_three_graphql_calls`
  + cache/stale tests; zero call-site edits — verify `harness/drive.rs`,
  `routes/board_goals.rs`, `routes/harness.rs`, `routes/mcp.rs` have NO diffs
  in this spec's commits.
- **AC 5:** `blocked_arm_moves_card_to_blocked_option_with_fake_gh` (no
  close/reopen on Blocked).
- **AC 6:** `done_closes_open_issue_and_skips_closed`,
  `in_progress_reopens_closed_only`, `knob_off_never_probes_closes_or_reopens`
  (probe-then-act both directions, knob-gated; knob default ON via ONE serde
  site `default_true`).
- **AC 7:** `github_transition_with_board_board_failure_is_skipped_note_still_ok`
  — board failure ⇒ `Ok(Skipped("status label applied; Projects board write
  failed: … gh auth refresh -s project"))`, never `Err`.
- **AC 8:** `github_transition_with_board_unbound_is_byte_identical` (today's
  exact 5-invocation log) + the F2 deletion audit (7 deletions, all intended;
  `github_transition_with`/`github_mark_blocked_with` byte-identical; zero
  test edits).
- **AC 9 (F3):** `create_repo_from_template` (probe ⇒ clone / create
  `--clone`), `provision_repo` four steps, wizard provision phase + template
  mode; consent checklist naming the exact 5 committed paths.
- **AC 10:** `provision_run_twice_changes_nothing` (run-twice: no second
  project, binding file byte-identical, scaffold changed:false, commit count
  equal) + `provision_with_existing_binding_never_creates_a_project` +
  `gitignore_rewrite_is_write_if_different_and_keeps_state_ignored` (real
  `git check-ignore`: engine state stays ignored, 5 contract paths trackable).
- **AC 11:** **DEFERRED to qa.sh / human demo (runner: Mateo)** — live
  custom-column board moves end-to-end. Not a tester-phase item (008
  precedent); tester rules only on the code-level ACs.

## Sacred surfaces — verify untouched (grep/diff, not trust)

`spawn_agent_into_pane` + autonomy mechanics; `github_transition_with` /
`github_mark_blocked_with` bodies; all four seam call-site files;
`TrackerPhase` (4 variants) / `TransitionResult` (no new variant, doc-only
edit); `useComposerState.ts`; `isGoalStepReady`/`GoalStepInputs`;
`initialComposerPhase`; `scaffold_harness`; desktop `gh.rs` write stubs;
`is_public` (auth.rs); `github_labels.rs`.

## Deviations to audit for ACCURACY against code (tasks.md)

F1 ×10 (top: local `gh_bin()` dup; paired-positive-guards for strict:false
narrowing) · F2 ×5 (top: second private `_with_board` fn for blocked-arm
testability; act-failure loud) · F3 ×10 (top: `Option<ProjectChoice>`
widening; `state_map` injection for hermeticity; `resolve_slug` +
`BLOCKED_LABEL` keep-in-sync duplications).

## Known sharp edges

- `ID_CACHE` is process-global in the test binary — any NEW test touching
  `board_write_with` must use a fresh (slug, number).
- The run-twice test uses real git in temp dirs (repo-local identity,
  `commit.gpgsign false`) — machine-config-proof by construction.
- `POST /api/workspace/provision` with `project` absent + no binding reports
  ok/changed:false "no project requested" — expected shape, not a failure.
- Full-vitest and bare-tsc are pre-existing dirty; only deltas count.

## Expected tester artifact

`ai/specs/010-end-to-end-autonomous-flow/verification.md` — independently
re-run every gate, PASS/FAIL per AC with repro steps, deviation-accuracy
audit, sacred-surface sweep; then `handoffs/04-tester-to-reviewer.md`.
