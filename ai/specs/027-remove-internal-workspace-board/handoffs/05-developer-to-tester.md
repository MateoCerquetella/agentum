# Handoff — Developer to Tester

- **Spec:** 027-remove-internal-workspace-board
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-23
- **Gate:** **PASS (retry iteration 1)**

## Summary

The Tester send-back is fixed at the shallowest harness seam. `qa.sh` can no
longer report browser QA green without evidence: it names every required Tasks
scenario, invokes a configured executable verifier with quoted arguments and
propagates its exact status, or reports `PENDING/UNAVAILABLE` and exits 2.

## Delivered

- Replaced `.agentum-harness/qa.sh`'s unconditional pass with the
  `AGENTUM_BROWSER_VERIFY_CMD` executable-wrapper contract.
- Named the GitHub-bound, Linear-bound, unbound, and unavailable-tracker Tasks
  scenarios and the required absence of internal-board cards and the
  `Sync to Board` action in both gate output and the verifier brief.
- Used `command -v -- "$verify_cmd"` plus quoted arguments and `exec`; no shell
  command string is evaluated, and verifier pass/fail propagates unchanged.
- Corrected `harness/types.rs` so `tracker_provider` advertises only GitHub and
  Linear while explicitly retaining arbitrary legacy string deserialization.
- Recorded retry implementation and exact evidence in `tasks.md`.
- Preserved the prior Tester send-back, `ai/STATE.md`, unrelated worktree edits,
  and the existing Vitest results cache file.

## Acceptance-criteria evidence

- **AC 1:** the focused `ProjectTasksPage.test.tsx` suite passes 4/4. The QA
  script now requires real-browser evidence for all four named Tasks states and
  absence of the retired internal-board affordance; without a configured runner
  it exits 2 rather than falsely satisfying this criterion.
- **AC 2:** unchanged by this retry; the previously green real-router 404 matrix
  remains the implementation evidence.
- **AC 3:** unchanged product behavior; the focused legacy-provider regression
  passes 1/1, and the comment-only Rust edit preserves `Option<String>`.
- **AC 4:** unchanged by this retry; no storage, migration, or compatibility
  behavior was edited.
- **AC 5:** focused Rust/UI checks, formatting, shell syntax, and diff checks
  pass. Real browser QA remains a truthful environment-dependent gate.

## Verification

- `bash -n .agentum-harness/qa.sh .agentum-harness/verify.sh` — **PASS**.
- Focused shell matrix using unset/unavailable commands and `/usr/bin/true` /
  `/usr/bin/false` verifier mocks — **PASS**; statuses were 2, 2, 0, and 1.
- Shell assertions for every named scenario plus `PENDING/UNAVAILABLE` —
  **PASS**.
- `env HARNESS_FEATURE_ID=F1 bash .agentum-harness/verify.sh` — **PASS**;
  1 UI file, 4 tests.
- `/Users/mateocerquetella/.cargo/bin/cargo test -p agentum-server --lib
  legacy_board_provider_is_non_mutating_and_best_effort` — **PASS**; 1/1.
- `/Users/mateocerquetella/.cargo/bin/cargo fmt --all -- --check` — **PASS**.
- `git diff --check` — **PASS**.

## Decisions and invariants

- `AGENTUM_BROWSER_VERIFY_CMD` is an executable path/name, not shell source. Its
  wrapper receives exactly two arguments: feature ID and the complete browser
  verification brief. This keeps whitespace and metacharacters inert.
- The existing boolean `AGENTUM_BROWSER_VERIFY` flag is not treated as a command;
  setting a capability flag alone cannot make this standalone shell gate pass.
- Exit 2 means the required real-browser verifier is unconfigured/unavailable;
  any configured verifier's own nonzero code is preserved by `exec`.
- No live GitHub/Linear mutations, product-code changes, release promotion,
  migration edits, legacy-value removal, or internal-board data deletion occurred.

## Pending work

- Tester should rerun the shell matrix and, where a real browser verifier wrapper
  plus safe project fixtures are available, execute the four scenarios. This
  environment has no configured `AGENTUM_BROWSER_VERIFY_CMD`, so real browser
  evidence remains **PENDING/UNAVAILABLE**, never PASS.

## Remaining risks / next action

- The command seam depends on the configured wrapper honoring the brief and
  returning nonzero for inconclusive or failed browser checks. The harness now
  propagates that outcome faithfully but cannot manufacture browser capability.
- Return to Tester for retry iteration 1 verification. Release and external
  tracker writes remain human-gated.
