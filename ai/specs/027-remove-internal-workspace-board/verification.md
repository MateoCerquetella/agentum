# Spec 027 — Tester verification

- **Date:** 2026-07-23
- **Role:** Tester
- **Iteration:** 2
- **Verdict:** PASS TO REVIEWER — automated evidence is green; real browser QA is explicitly PENDING/UNAVAILABLE for the environment/release gate

## Summary

The Developer retry fixes the iteration-1 false-positive browser gate. The
runtime `qa.sh` names all four required Tasks states and the absence of the
retired board affordance, invokes a configured verifier as exactly two quoted
arguments, propagates its status through `exec`, and exits 2 with
`PENDING/UNAVAILABLE` when the verifier is unset or missing. Independent
results are unset/missing/true/false = 2/2/0/1. Metacharacter probes remained
inert. `/usr/bin/true` and `/usr/bin/false` verify wrapper propagation only;
neither is represented as browser evidence.

The focused Tasks suite passes 4/4 and the legacy-provider Rust regression
passes 1/1 after the retry. Shell syntax, Rust formatting, current/feature diff
checks, harness wiring, and the external-only provider comment all pass. The
retry changes no product behavior, so the same-turn independent iteration-1
F2–F4 results and two fresh F5 executions remain applicable: both full runs
passed 901 workspace library tests with 6 expected ignores and 0 failures, and
both production builds transformed 7,253 modules and exited 0.

No Playwright MCP or configured `AGENTUM_BROWSER_VERIFY_CMD` exists in this
environment. Real-browser verification of GitHub-bound, Linear-bound, unbound,
and unavailable Tasks states therefore remains **PENDING/UNAVAILABLE**, never
green, and is deferred to a browser-capable environment/release gate. Under the
Tester role, every AC has automated evidence and the unavailable runtime leg is
now an explicit environment deferral rather than a false pass, so the spec may
advance to Reviewer.

## Acceptance-criteria evidence

| AC | Automated, inspected, and runtime evidence | Verdict |
|---|---|---|
| 1 | Retry F1 passes `ProjectTasksPage.test.tsx` (1 file, 4 tests). Source dispatches only configured GitHub/Linear views, renders explicit unbound/unavailable copy, and has no internal-board client or sync affordance. Both production F5 builds pass. `qa.sh` now names all four real-browser scenarios and refuses to pass without a verifier. | **PASS automated; real browser PENDING/UNAVAILABLE at environment/release gate** |
| 2 | Iteration-1 F2 independently passed the real-router 404 matrix 1/1 for all five retired family roots and representative nested paths. The retry touched no route or router code; the full workspace suite remains green. | **PASS** |
| 3 | Iteration-1 F3 independently passed both focused sink regressions; retry rerun of `legacy_board_provider_is_non_mutating_and_best_effort` passes 1/1. `TaskSink` remains GitHub/Linear-only, tracker seams have no board-store path, and the updated comment advertises external providers while retaining legacy string deserialization. | **PASS** |
| 4 | Iteration-1 F4 independently passed legacy-row reopen/inertness 1/1 and watchdog 11/11. The retry changes only the provider comment and runtime QA wrapper; migrations, compatibility storage, and normal flows are unchanged. | **PASS** |
| 5 | Developer and independent Tester F5 executions each passed 901 Rust library tests, 6 ignored, 0 failed; Vite transformed 7,253 modules and exited 0 (1m29s and 1m18s respectively). Retry F1/Rust plus syntax/fmt/diff/wiring checks are green. | **PASS** |

## Browser QA contract and deferral

- Unset `AGENTUM_BROWSER_VERIFY_CMD` exits 2 and prints
  `PENDING/UNAVAILABLE` plus “No browser scenarios were verified; refusing to
  pass.”
- A missing verifier path exits 2 and prints `PENDING/UNAVAILABLE`.
- `/usr/bin/true` exits 0 and `/usr/bin/false` exits 1, proving exact status
  propagation by `exec`.
- The wrapper prints requirements for GitHub-bound, Linear-bound, unbound, and
  unavailable-tracker Tasks states, plus no internal-board cards or
  `Sync to Board` action.
- Source inspection pins `exec "$verify_cmd" "$feature_id"
  "$verification_brief"`. A `printf` boundary probe received the complete brief
  as one argument, and command/feature metacharacter probes created no marker,
  confirming there is no `eval` or shell-string execution.
- No Playwright/browser tool and no real verifier command are available here.
  Consequently, no browser scenario was executed and none is labeled PASS.

## Independent retry commands and results

- `env -u AGENTUM_BROWSER_VERIFY_CMD HARNESS_FEATURE_ID=F1 bash .agentum-harness/qa.sh` — **PENDING/UNAVAILABLE**, exit 2.
- Missing verifier path through `AGENTUM_BROWSER_VERIFY_CMD` — **PENDING/UNAVAILABLE**, exit 2.
- `/usr/bin/true` / `/usr/bin/false` verifier mocks — wrapper statuses **0/1**.
- Exact two-argument, complete-brief, named-scenario, and metacharacter assertions — **PASS**.
- `env HARNESS_FEATURE_ID=F1 bash .agentum-harness/verify.sh` — **PASS**; 1 file, 4 tests.
- `/Users/mateocerquetella/.cargo/bin/cargo test -p agentum-server --lib legacy_board_provider_is_non_mutating_and_best_effort` — **PASS**; 1/1, 701 filtered out.
- `bash -n .agentum-harness/qa.sh .agentum-harness/verify.sh` — **PASS**.
- `/Users/mateocerquetella/.cargo/bin/cargo fmt --all -- --check` — **PASS**.
- `git diff --check` and `git diff 84c5a1c7^..HEAD --check` — **PASS**.
- F1–F5 JSON wiring and external-only `tracker_provider` comment assertions — **PASS**.
- Results-cache SHA-256 remained
  `2a07ee7f1771dcd8ff2d2e0bf6f4d984413e02a85e30298dcf86c2df630dcc83`.

## Runtime-ignore finding

`git check-ignore -v .agentum-harness/qa.sh` resolves to
`.agentum-harness/.gitignore:1:*`, and `git ls-files` confirms `qa.sh` is
untracked. This does not invalidate the current gated run: the runtime harness
file exists in the shared worktree and was the exact artifact executed and
inspected. It does affect portability and review visibility: the QA fix is not
present in `git diff` and will not travel in a commit or fresh checkout.
Reviewer must inspect the current runtime artifact and preserve or regenerate
the truthful wrapper through the harness orchestration path; Reviewer must not
infer from the source diff alone that a newly generated `qa.sh` has this
contract. This is a bounded harness-lifecycle risk, not a product-code blocker.

## Remaining risks / release gate

- Execute the four named Tasks scenarios in a real browser using safe GitHub,
  Linear, unbound, and unavailable fixtures before release; any inconclusive
  scenario must keep the gate nonzero.
- Do not treat mock-wrapper status propagation as visual evidence.
- Preserve/regenerate the ignored runtime `qa.sh` contract for Reviewer and any
  later harness run.
- Live external tracker writes and release promotion remain human-gated.

The only observed Rust warning is the pre-existing `forge_send` dead-code
warning. The requested Vitest results cache content was preserved.
