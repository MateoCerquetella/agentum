# Verification — spec 010 (tester)

> Independent verification by the sdd-tester (autonomous /sdd-loop), written
> verbatim by the orchestrator. Verdict: **PASS-WITH-DEFERRALS** (AC 11 only).

## Environment

- Worktree: `.claude/worktrees/prd-agentum-end-to-end-autonomous`, HEAD
  `bc4a7310`, tree clean.
- Slice commits verified present: F1 `474cfd12`, F2 `0b03eb9e`, F3 `26b1e022`.
- Pre-spec base: `664ee365` ("Merge origin/develop (v0.59.0)"). The mid-spec
  v0.59.1 merge (`e271d833`) touched only desktop sidebar/ui-store files — no
  sacred surface — so all base..HEAD diffs on sacred files are attributable to
  the spec commits alone.
- Date: 2026-07-06. macOS (darwin 25.2.0).

## A. Gate re-runs (all independent)

| # | Gate | Command | Expected | Actual | Verdict |
|---|---|---|---|---|---|
| 1 | Rust unit | `cargo test -p agentum-server --lib` | 616 / 0 / 5 ignored | `616 passed; 0 failed; 5 ignored` (93.6 s) | PASS |
| 2 | Fmt | `cargo fmt --all --check` | clean | `FMT-CLEAN` | PASS |
| 3 | Clippy | `cargo clippy --workspace` | 0 warnings | exit 0; full-output `grep -c "^warning"` = 0 | PASS |
| 4 | UI build | `NODE_OPTIONS=--max-old-space-size=3072 npm run build` (ui/) | green | `✓ built in 1m 48s` (chunk-size warning pre-existing) | PASS |
| 5 | Vitest (3 suites) | `npx vitest run workspace-provision-step… workspace-goal-step… github-projects-binding…` | 37 / 0 | `Test Files 3 passed; Tests 37 passed` (587 ms, no flake — no timeout retry needed) | PASS |
| 6 | tsc baseline | `npx tsc --noEmit -p tsconfig.json \| grep -c "error TS"` | exactly 1642 | 1642 | PASS |

Targeted suite runs: `github_projects` filter 30/0, `provision` filter 32/0,
`task_sink` filter 35/0/1-ignored — all three F2 seam tests listed green by
name.

## B. AC rulings

**AC 1 — PASS.** `crates/agentum-server/src/github_projects.rs`:
`StatusMapping` has five REQUIRED `String` fields (unmapped phase
unrepresentable by type); `BoardBinding {project_id, status_field_id,
status_mapping, done_closes_issue #[serde(default="default_true")]}`;
persistence = `upsert_binding_at`/`binding_for_slug_at`/`remove_binding_at`
under a `WRITE_LOCK`'d RMW keyed by lowercase slug. Discovery is one
`gh api graphql` call (`discover_status_field` → `run_gh_graphql`); fuzzy
exact-normalized match with exactly the two fallbacks (RTT→InProgress,
Blocked→InProgress). Test evidence read, not just run:
`stored_binding_missing_phase_fails_deserialize_reads_as_no_binding` (a file
missing `blocked` reads as `None`), `binding_for_slug_is_case_insensitive`,
`upsert_preserves_other_slugs` (RMW + remove semantics). Repro:
`cargo test -p agentum-server --lib github_projects`.

**AC 2 — PASS.** All four fixtures exist and assert the demanded shapes
(bodies read): (a) `resolve_default_board_maps_three_and_falls_back_rtt_blocked`
— Todo/In Progress/Done matched, RTT & Blocked `FellBack` to the InProgress
option; (b) `resolve_custom_backlog_building_qa_shipped` — ReadyToTest→"QA",
Done→"Shipped" by id AND name; (c)
`resolve_no_rtt_column_falls_back_to_in_progress_option` — falls back to a
custom-named "🚧 In-Progress" option, flagged FellBack; (d)
`classify_scope_missing_names_gh_auth_refresh` (both classifier inputs →
`scope_missing` with the literal `gh auth refresh -s project` remedy) +
`parse_discovery_missing_status_field_is_actionable` (missing/non-single-select
Status → `no_status_field`; null project → `not_found`) +
`resolve_refuses_when_core_phase_unmappable_never_partial` (refusal names
phases + options; substring non-match pinned: "Not Started"/"Not Done" never
match) + `discover_status_field_with_fake_gh` end-to-end.

**AC 3 — PASS (code-level).** `ProjectBindingEditor.tsx`: per-phase selects
(`BOARD_PHASES.map` → `<select>` at :356/:362), FellBack hints
(`fallbackHints` — names the fallback option and the "Add one on GitHub and
re-discover" recovery), refusal → all-empty selects + manual completion
(`selectionFromResolved(null)` all-empty, unit-tested), Save (PUT) /
Re-discover (:468) / Unbind (:288/:477), project pick via the existing read
commands `api.gh.listAccessibleProjects()`/`api.gh.resolveProjectRef({input})`.
Mounted wizard-independently in `IntegrationsPane.tsx`
(`GithubProjectsBoardEditor` :234, rendered :590) and second-mounted by F3's
`NewWorkspaceProvisionStep.tsx` :158 (D7). Visual rendering itself belongs to
the AC 11 demo.

**AC 4 — PASS.** The arm lives inside the seam (`task_sink.rs` github arms
call the new private `_with_board` fns after the URL parse);
`board_write_with_fake_gh_cold_is_three_graphql_calls` pins node-id query →
`addProjectV2ItemById` → `updateProjectV2ItemFieldValue` with
`-f option=opt-rtt` (IDs, never names); cache + stale-heal tests pin warm/heal
call counts. **Zero call-site edits proven**: `git diff 664ee365..HEAD --stat`
is empty for `harness/drive.rs`, `routes/board_goals.rs`, `routes/harness.rs`,
`routes/mcp.rs`.

**AC 5 — PASS.** `blocked_arm_moves_card_to_blocked_option_with_fake_gh`
(task_sink): today's label+comment path (lines 0–2 of the log pinned exactly)
then 3 GraphQL lines ending in `-f option=opt-blocked`, and asserts **no**
`issue view`/`close`/`reopen` even with the knob ON.

**AC 6 — PASS.** `done_closes_open_issue_and_skips_closed` (probe + close on
OPEN; probe-only on CLOSED), `in_progress_reopens_closed_only` (symmetric),
`knob_off_never_probes_closes_or_reopens` (no `issue view` at all when off).
Default-ON at ONE serde site: `default_true` +
`done_closes_issue_defaults_true_when_absent` (absent knob reads ON; explicit
false round-trips). Unbound flows keep today's contract (AC 8 test).

**AC 7 — PASS.** `github_transition_with_board_board_failure_is_skipped_note_still_ok`:
fake gh fails **only** `$1 = "api"`; asserts the label path fully ran (4
ensures + issue edit), the result is `Skipped` starting
`"status label applied; Projects board write failed:"` and containing the
scope remedy — returned inside `Ok` by the arm; `tracing::warn` in the fold
(task_sink.rs `github_transition_with_board`).

**AC 8 — PASS.** `github_transition_with`/`github_mark_blocked_with` bodies
extracted from `664ee365` and HEAD and compared: **byte-identical**.
Cumulative task_sink deletions base..HEAD = 7 (Skipped docstring, 2 arm
comment lines, 2 arm caller lines from F2; 2 signature-widening lines from
F3) — all documented; F2's own slice = 5 in task_sink + 2 in github_projects
(the runner refactor), matching the claimed 7. Zero existing-test edits
(every test hunk is an addition).
`github_transition_with_board_unbound_is_byte_identical` pins the exact
5-invocation sequence today's `github_transition_applies_with_fake_gh` pins,
plus `!calls.contains("api graphql")`.

**AC 9 — PASS.** `provision.rs::create_repo_from_template`: local `.git` ⇒
`created:false`; `gh repo view` probe exists ⇒ clone; missing ⇒
`repo create --template … --clone`; post-condition that the clone landed; gh
stderr verbatim (400-char bound). `provision_repo` four independent
best-effort steps: own 5-label ensure over the two widened builders;
link-or-create **guarded by the existing binding**; `scaffold_harness`
wrapped untouched; consent-gated commit staging exactly the 5 `COMMIT_PATHS`,
porcelain-empty ⇒ no commit, plain `push origin HEAD` (never `--force`), no
AI trailer, red push non-fatal (`provision_red_push_is_nonfatal_and_reported`).
Wizard: template mode with editable owner/name/template (D4 default constant
pinned in vitest), the modal-local `'provision'` phase, the D8 consent
checklist rendering `provisionCommitFileList()` (the exact 5 paths,
vitest-pinned incl. "never lists engine state") with a default-ON toggle
naming "the project's current branch", Skip always available.

**AC 10 — PASS.** `provision_run_twice_changes_nothing` (details in E),
`provision_with_existing_binding_never_creates_a_project` (a request's
`Create` is ignored when bound: no `project create`, no graphql, binding file
byte-untouched), `gitignore_rewrite_is_write_if_different_and_keeps_state_ignored`
(real git proof). Labels idempotent by `--force` contract;
`provision_skips_commit_when_consent_off` also pins the blanket `*` staying
untouched (§6.8).

**AC 11 — PASS (deferred).** Live custom-column board demo = qa.sh / human
demo, runner Mateo (008 AC 12 precedent). Not a tester-phase item; evidence
contract = issue timeline events + a demo-pass line in `ai/STATE.md`.

## C. Sacred-surface sweep

| Item | Proof | Result |
|---|---|---|
| 4 seam call-site files (`harness/drive.rs`, `routes/board_goals.rs`, `routes/harness.rs`, `routes/mcp.rs`) | `git diff 664ee365..HEAD --stat -- <f>` | empty ×4 — CLEAN |
| `useComposerState.ts` | same | CLEAN |
| `github_labels.rs` | same, **at the real path** `crates/agentum-desktop/src/commands/github_labels.rs` (the handoff spelled it without `commands/` — a diff at that path passes vacuously; re-proved at the real path) | CLEAN |
| Desktop `commands/gh.rs` | diff empty + read: `gh_update_project_item_field`/`gh_clear_project_item_field` still `not_available()` at :1046/:1051 | CLEAN |
| `auth.rs` (`is_public`) | diff empty | CLEAN |
| `harness/types.rs` (incl. `scaffold_harness`, `FeatureList`) | diff empty | CLEAN |
| Spawn path / autonomy mechanics | `routes/sessions.rs` + `routes/sessions/` diff empty; no spec commit touches any session/spawn file | CLEAN |
| `github_transition_with` / `github_mark_blocked_with` | fn bodies extracted from base and HEAD, string-compared | BYTE-IDENTICAL |
| task_sink.rs only-allowed hunks | full diff read: Skipped docstring, 2 `pub(crate)` widenings, 2 private `_with_board` fns, 2 arm hooks (binding read after parse), test additions — nothing else | CONFIRMED |
| `TrackerPhase` | enum read: Todo/InProgress/ReadyToTest/Done | exactly 4 variants |
| `TransitionResult` | enum read: Applied, Skipped(String) | no new variant; doc-only edit |
| Desktop read commands `gh_projects.rs` + `routes/github.rs` | diff empty | CLEAN |
| `isGoalStepReady`/`GoalStepInputs`/`initialComposerPhase` | full `workspace-goal-step.ts` diff read: only doc comment + type widening + appended 4th step | UNTOUCHED |

## D. Deviation audit (25/25)

**F1 (10):** 1 test-name `…_big_f_for_ints` — ACCURATE (exists at :1090,
behavior pinned). 2 path-injected cores — ACCURATE
(`binding_for_slug_at`/`upsert_binding_at`/`remove_binding_at`, all
`pub(crate)`, public fns delegate). 3 `gh_bin()` dup — ACCURATE
(github_projects.rs:414–418 with the cross-link comment; same `AGENTUM_GH_BIN`
knob). 4 `unmapped_core_phases` — ACCURATE (pure, tested at :1084, feeds the
route's `unmappedPhases`). 5 `parseProjectInput` not imported — ACCURATE
(editor calls `api.gh.resolveProjectRef({input})` at :184; no import). 6
classified non-scope → 400 envelope — ACCURATE (`projects_error_to_api`:
scope_missing→422 else 400, same `{error:{code,message}}` shape). 7 in-file
section component — ACCURATE (`GithubProjectsBoardEditor` in
IntegrationsPane.tsx:234). 8 `selectionForRebind`+`optionNamesForSelection` —
ACCURATE (exported, 8 test references). 9 knob toggle PUTs immediately when
bound — ACCURATE (:330 branches bound→`handleToggleDoneCloses`). 10 paired
positive guards — ACCURATE (`if (res.ok)`/`if (res.ok === false)` at
:121/:125 and :187/:191).

**F2 (5):** 1 two private seam fns — ACCURATE (both `async fn`, no `pub`;
label fns untouched). 2 `run_gh_graphql` = one-line wrapper over
`run_gh_graphql_argv` — ACCURATE (:508–515). 3 probe `--jq .state` +
defensive parse — ACCURATE (argv pinned in tests; `trim().trim_matches('"')`
+ uppercase at :921). 4 act failure `Err(reason)`, probe failure warn+skip —
ACCURATE (`close_or_reopen_for`). 5 std `LazyLock` — ACCURATE (:18/:778).

**F3 (10):** 1 crate-root `provision.rs` — ACCURATE (F3 commit does not touch
`github_projects.rs`). 2 `project: Option<ProjectChoice>` — ACCURATE
(ProvisionCtx:241; None = "no project requested" ok/changed:false). 3
`state_map` injection — ACCURATE (:248; route `from_env()`, tests `Default`).
4 `BLOCKED_LABEL` dup + keep-in-sync — ACCURATE (:24–28; task_sink's
`GITHUB_BLOCKED_LABEL` :279 is private and value-identical). 5 test name
`…_frozen_fixture` — ACCURATE (:752). 6 own `run_in` (120 s, 400-char
verbatim, caller cwd) — ACCURATE (:33/:120–149). 7 labels `changed` always
false — ACCURATE (`StepReport::ok(false, "ensured…")`). 8 `resolve_slug` copy
+ keep-in-sync comment — ACCURATE (routes/provision.rs:36–39). 9 generic
branch naming pre-run, authoritative post-run — ACCURATE (ProvisionStep :217
"the project's current branch"; `CommitReport.branch` from `rev-parse`). 10
header copy — ACCURATE (NewWorkspaceGoalStep :189).

No deviation misdescribes the code.

## E. Adversarial spot-checks

1. **AC-7 pin**: the fake gh fails ONLY when `$1 = "api"` (GraphQL); every
   label call exits 0. Asserts the `Skipped` prefix AND
   `contains("gh auth refresh -s project")` AND that the label path fully ran
   (4 `label create status/` + `issue edit 42`). Airtight.
2. **Run-twice**: run 2 is isolated by `skip(run1_lines)` over the gh log and
   asserts **both** no `project create` and no `api graphql`; commit equality
   is real `git rev-list --count HEAD` (not report fields); binding file
   compared byte-for-byte; fixture uses repo-local identity +
   `commit.gpgsign false` + a bare origin — machine-config-proof as claimed.
3. **Gitignore rewrite**: real `git check-ignore -q` proves
   `feature_list.json`, `handoff.md`, `qa/verdict.json` stay ignored AND
   iterates all 5 `COMMIT_PATHS` asserting NOT ignored; write-if-different
   pinned by the second-call no-op; blanket `*` absence asserted line-exactly.
4. **`binding_for_slug` hermeticity**: `AGENTUM_GITHUB_PROJECTS_CONFIG` has
   zero references in task_sink.rs (tests never set it); the binding read sits
   at task_sink.rs:855, strictly after the two early-return guards (:838
   no-url, :843 unparseable); the only github-arm tests through the seam pass
   None/blank/pull-URL — none can reach the real config read. Confirmed also
   for `apply_blocked_transition`'s tests.
5. **Unbound byte-identical**: assert-for-assert mirror of
   `github_transition_applies_with_fake_gh` (5 lines; first 4
   `label create status/`…`--force`; line 5
   `issue edit 42 --repo owner/repo --add-label status/in-progress`) plus the
   added `no GraphQL` assert.

## Defects

None.

## Info nits (non-blocking)

1. **tasks.md F3 vitest per-file counts are swapped**: it claims
   `workspace-provision-step.test.ts` has "15 tests" and goal-step 12;
   reality is provision-step **12**, goal-step **15** (binding 10; total 37
   correct, substance of the goal-step edit — only the steps pin — correct).
   Also "5 vitest describe-blocks" — the provision-step file has 4.
2. **Handoff sacred-list path spelling**: `github_labels.rs` lives at
   `crates/agentum-desktop/src/commands/github_labels.rs`; the handoff's
   spelling without `commands/` makes a naive `git diff -- <path>` pass
   vacuously. Verified at the real path (clean).
3. The F3 "run-twice ran RED against a stubbed commit step first" test-first
   claim is session-internal — not reconstructable from git (one commit per
   slice). The tests' present strength stands independently.
4. Full-vitest (~31 pre-existing failing files) not re-run — outside the
   mandated gates; tsc delta (gate 6) covers the baseline-regression question
   for this spec's files.
5. `ID_CACHE` sharp edge honored: every F2 test uses a unique slug.

## Verdict

All six gates reproduce exactly; ACs 1–10 PASS on independently-read
evidence; sacred surfaces proven untouched (label-path fns byte-identical);
all 25 deviations accurate; five adversarial spot-checks clean; zero defects.

**PASS-WITH-DEFERRALS** — deferral: AC 11 (live custom-column board demo,
qa.sh / human-run, runner Mateo; evidence = issue timeline project-status +
close events, demo-pass line in `ai/STATE.md`).
