# Handoff — Tester to Developer

- **Spec:** 027-remove-internal-workspace-board
- **From:** Tester
- **To:** Developer
- **Date:** 2026-07-23
- **Gate:** **SEND-BACK (iteration 1)**

## Delivered

- Independent `verification.md` mapping all five acceptance criteria to
  focused, full-suite, source-inspection, negative, error, and legacy evidence.
- Exact F1–F4 harness gates, full F5 gate, formatting, shell syntax, JSON wiring,
  and changed-diff checks rerun independently.
- Product implementation supported by green evidence; no product code or
  harness script was modified during Tester.

## Gate result

- F1: **PASS** — 1 UI file, 4 tests.
- F2: **PASS** — real-router 404 matrix 1/1.
- F3: **PASS** — external-only creation 1/1 and legacy provider 1/1.
- F4: **PASS** — legacy-row inertness 1/1 and watchdog 11/11.
- F5: **PASS** — 901 workspace library tests passed, 6 ignored, 0 failed;
  desktop build passed with 7,253 modules transformed.
- Browser QA: **FAIL BY CONTRACT** — `qa.sh` exits 0 after printing
  `qa: no browser checks configured — passing`; it performs none of the Tasks
  checks promised by the spec. Playwright MCP browser tooling is absent, so the
  real browser leg is unavailable/deferred and must not be green.

## Shallowest required fix

Wire `.agentum-harness/qa.sh` to the four required Tasks scenarios: GitHub-bound,
Linear-bound, unbound, and unavailable, with no internal-board sync action in
any state. The script must invoke and propagate a real browser verifier when
available. Without the required browser runner/Playwright MCP, it must clearly
report PENDING/UNAVAILABLE and exit nonzero (such as 2) instead of passing.

Also correct the stale `harness/types.rs` tracker-provider comment so it
advertises GitHub/Linear while preserving legacy string deserialization. This is
documentation cleanup, not a request to remove the legacy compatibility path.

## Decisions and invariants

- Acceptance criteria and architecture remain unchanged.
- Do not claim browser QA from structural tests or from the current no-op.
- Do not delete historical migrations or legacy board rows.
- Preserve external GitHub Projects, Linear kanban, harness-board terminology,
  and all external-tracker transition behavior.
- Do not perform live tracker writes or release promotion as part of this fix.

## Return condition

Return to Tester after the QA route no longer false-passes, its syntax/diff
checks are green, and its unavailable-environment behavior is reproducible.
Real browser execution may remain an explicit environment/release deferral, but
the harness must represent that truthfully.
