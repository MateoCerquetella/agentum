# Spec 026 — Implementation tasks

## F1 — Binding identity fidelity

- Complete and verify the existing repo-aware compatibility projection in
  `routes/project_trackers.rs`.
- Require canonical `Repo.id` ownership plus normalized equality between the
  stored GitHub target and `resolve_tracker_slug` output.
- CAS-repair migrated mismatches from the exact repo origin; preserve configured
  mismatches and return `tracker_target_mismatch`.
- Add matching, migrated-repair, configured-preservation, two-repo isolation,
  local, SSH, and missing-host regression tests.
- Covers AC 1, 3, 6, and 7.

Gate: focused `cargo test -p agentum-server project_trackers --lib` (or the
smallest exact test filters), followed by `git diff --check`.

## F2 — Wizard closed tracker scope

- Carry the binding response's resolved slug into `PickerBindingResolution`.
- Key binding/table/cache eligibility and late-response acceptance by selected
  repo target + resolved slug + GitHub Project identity.
- Filter Project rows by the resolved repository slug before grouping, counting,
  searching, displaying, or selecting them.
- Render absent/configured-mismatch/host-failure states without a connected badge
  or rows, while keeping workspace creation optional and exposing the relevant
  configure/reconfigure action for local and SSH repos.
- Pin the existing repo-switch linked-item reset and exact linked/unlinked
  worktree coordinates.
- Covers AC 2, 4, 5, 7, and 8.

Gate: focused Vitest for both New Workspace model suites and the runtime client,
`npm run build --prefix crates/agentum-desktop/ui`, relevant server library
tests, and `git diff --check`.

## Developer execution — 2026-07-21

- **F1 implementation:** COMPLETE in the existing canonical compatibility seam.
  Matching slugs project the repo-owned row; migrated mismatches CAS-delete and
  re-migrate from the resolved origin; configured mismatches return
  `tracker_target_mismatch` without modifying the canonical row. Focused tests
  cover matching/two-repo isolation, explicit preservation, and migrated repair;
  existing `routes::util` tests pin unknown/missing repo identity without a
  local fallback.
- **F2 implementation:** COMPLETE. The response slug now remains attached to a
  resolved binding, classified mismatch failures have a dedicated reconfigure
  state, table eligibility and late completions use the full
  repo-target + slug + Project key, and row derivation filters by normalized
  repository slug before grouping/counting/searching/selection. Configure is
  available for local and SSH git repos through the same `workdir + repoId`
  editor path. Existing linked/unlinked persistence seams remain unchanged.

### Gate results

- `bunx vitest run src/components/new-workspace/work-item-picker-model.test.ts src/components/new-workspace/create-workspace-wizard-model.test.ts src/runtime/github-projects-client.test.ts` (from UI package) — **PASS**, 3 files / 68 tests.
- `git diff --check` — **PASS**.
- `rustfmt --edition 2024 --check crates/agentum-server/src/routes/project_trackers.rs` — **PASS**.
- `cargo fmt --all -- --check` — **BLOCKED by unrelated pre-existing diff** in
  `crates/agentum-executor/src/adapters.rs` (import ordering only; untouched).
- `cargo test -p agentum-server project_trackers --lib -- --nocapture` —
  **PASS**, 5 passed / 0 failed (warm-cache orchestrator rerun; 1m27s compile,
  2.26s tests). One pre-existing duplicate test-attribute warning remains in
  `src/github_projects.rs`.
- `npm run build --prefix crates/agentum-desktop/ui` — **PASS**, Vite production
  build completed in 2m20s. Existing dynamic-import and chunk-size warnings are
  informational.

**Developer gate: PASS.** F1 and F2 are code-complete and eligible for Tester.

## Tester send-back — iteration 1 (2026-07-21)

Focused implementation gates remain green, but Tester returned the work for
missing executable coverage:

- add both Spec 026 feature IDs and exact command routing to
  `.harness/feature_list.json`, `.harness/verify.sh`, and `.harness/qa.sh`
  without replacing existing AutoWiki features;
- add a component regression for deferred A responses after switching to B on
  the same GitHub Project; and
- add repo-switch plus linked/unlinked create-coordinate persistence coverage.

See `verification.md` and `handoffs/04-tester-to-developer.md`. Real desktop and
SSH QA remains explicitly unrun because this worktree has no verified
current-build desktop instance or named safe fixtures.

## Developer retry — iteration 2 (2026-07-21)

- Preserved all three AutoWiki features and added
  `binding-identity-fidelity` plus `wizard-closed-tracker-scope` to
  `.harness/feature_list.json`.
- Added exact feature routing in `.harness/verify.sh`: focused tracker/host Rust
  tests for F1; focused New Workspace/runtime/scope tests, exact coordinate
  persistence test, Vite build, and diff check for F2.
- Added honest `.harness/qa.sh` routing for both features. Until the named live
  desktop/local/SSH matrix is actually run, each route reports `PENDING` and
  exits 2 rather than manufacturing a pass.
- Added the TrackerSection async commit-boundary regression: defer A, switch to
  B on the same Project, commit B, then resolve A and assert B's status, count,
  and rows remain byte-equivalent.
- Added repo-switch linked-item clearing coverage and linked versus unlinked
  worktree-create payload coverage for exact versus absent tracker coordinates.

### Retry gate results

- Harness JSON + shell syntax — **PASS** (`jq empty`; `bash -n` for verify/QA).
- `HARNESS_FEATURE_ID=binding-identity-fidelity bash .harness/verify.sh` —
  **PASS** (5 tracker tests, 4 host-resolution tests, diff check).
- `HARNESS_FEATURE_ID=wizard-closed-tracker-scope bash .harness/verify.sh` —
  **PASS** (4 files / 70 tests; exact worktree test 1 passed / 81 filtered;
  Vite production build 1m42s; diff check).
- Both Spec 026 QA feature routes — **PASS as routing checks**: each reports
  `PENDING` and exits 2. Live QA itself remains unrun and is not claimed.
- Final `git diff --check` — **PASS**.

**Developer retry gate: PASS.** Return iteration 2 to Tester; live desktop QA
remains a Tester/release evidence requirement, not a fabricated unit result.

## Reviewer retry — iteration 2 (2026-07-21)

- Added `ProjectBindingEditor.onUnbound`, a typed success-only callback fired
  after the repo-owned binding DELETE succeeds and editor state is cleared.
- TrackerSection handles that callback synchronously for the current
  `bindingTargetKey`: the latest eligible scope becomes null, resolution becomes
  `absent`, table/status/query state clears, and the editor closes. Old table
  completions are rejected before another render can accept them.
- Added a production-seam regression proving successful unbind yields
  `Configure tracker`, status `none` (no connected badge), zero eligible rows,
  and rejection of a late completion carrying the deleted scope.
- Corrected stale TrackerSection comments: selected git repos never fall back to
  global Project state, and the repoId-aware editor supports SSH configuration.

### Reviewer retry gate results

- `bunx vitest run src/components/new-workspace/tracker-section-scope.test.ts`
  — **PASS**, 2 tests.
- `HARNESS_FEATURE_ID=wizard-closed-tracker-scope bash .harness/verify.sh` —
  **PASS** (4 files / 71 tests; exact worktree test 1 passed / 81 filtered;
  Vite production build 1m11s; diff check).
- Final `git diff --check` — **PASS**.

**Reviewer retry gate: PASS.** Return to Tester/Reviewer for independent B1
confirmation. Live desktop/SSH QA remains pending and unclaimed.

## Reviewer send-back — iteration 1 (2026-07-21)

Final review found one code blocker: a successful inline unbind clears only
`ProjectBindingEditor` state, leaving `TrackerSection` connected to the deleted
binding until repo change/remount. Add a typed unbind notification, set the
current parent binding resolution to `absent` synchronously, and test that the
same mounted wizard immediately shows Configure tracker with zero rows and no
connected badge. See `review.md` and
`handoffs/05-reviewer-to-developer.md`.

## Final QA

- In the real desktop, view Agentum then select unbound `xcode-theme`: capture
  Configure tracker, zero Agentum rows, and no connected badge.
- Bind `xcode-theme` to a mixed-repository Project and confirm only
  `xcode-theme` issues appear.
- Switch repeatedly between the two repos, including while requests are in
  flight; confirm status/count/list never flash across scopes.
- Repeat bound/unbound behavior for an SSH repo and verify missing-host failure
  stays closed.
- Create one linked and one unlinked workspace and inspect their persisted
  tracker coordinates.
