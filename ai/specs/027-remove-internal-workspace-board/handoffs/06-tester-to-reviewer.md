# Handoff — Tester to Reviewer

- **Spec:** 027-remove-internal-workspace-board
- **From:** Tester
- **To:** Reviewer
- **Date:** 2026-07-23
- **Gate:** **PASS (Tester iteration 2)**

## Delivered

- Updated `verification.md` with iteration-2 AC mapping, negative/error cases,
  exact retry evidence, and the explicit browser environment/release deferral.
- Independently reproduced the truthful QA wrapper matrix: unset verifier 2,
  missing verifier 2, `/usr/bin/true` 0, `/usr/bin/false` 1.
- Verified the five named browser requirements, exact quoted two-argument
  command/brief contract, complete brief boundary, status propagation, and
  shell-metacharacter safety.
- Reran focused F1 (4/4), the legacy-provider Rust regression (1/1), shell
  syntax, formatting, diff, JSON wiring, and provider-comment checks.
- Preserved `node_modules/.vite/vitest/.../results.json` byte-for-byte.

## Acceptance-criteria evidence

- **AC 1:** focused Tasks suite passes 4/4; both production builds pass. The QA
  wrapper now refuses false-green results and names GitHub, Linear, unbound,
  unavailable, and retired-affordance checks. Real browser execution remains
  explicitly **PENDING/UNAVAILABLE** in this environment.
- **AC 2:** independent real-router 404 matrix passes 1/1; retry changed no
  router code.
- **AC 3:** external-only sink checks are green; retry legacy-provider test
  passes 1/1 and the comment now advertises GitHub/Linear without narrowing the
  legacy wire type.
- **AC 4:** independent legacy-row inertness 1/1 and watchdog 11/11 remain
  applicable; retry changed no storage, migration, or worker code.
- **AC 5:** two same-turn F5 runs passed 901 Rust library tests, 6 ignored, 0
  failed, and both Vite builds transformed 7,253 modules with exit 0. Retry
  syntax/fmt/diff/focused checks are green.

## Verification

- QA unset/missing/true/false matrix — **PASS** (2/2/0/1).
- QA named-scenario, exact argv/brief, and shell-safety assertions — **PASS**.
- `env HARNESS_FEATURE_ID=F1 bash .agentum-harness/verify.sh` — **PASS** (4/4).
- Focused legacy-provider Rust test — **PASS** (1/1).
- `bash -n`, `cargo fmt --all -- --check`, current/feature `git diff --check`,
  JSON wiring, and provider-comment assertions — **PASS**.
- Real Playwright/browser execution — **PENDING/UNAVAILABLE**, not PASS; no
  browser tool or configured verifier is present.

## Decisions and invariants

- Explicit environment/release deferral is acceptable at this Tester gate
  because all ACs have automated evidence and the unavailable browser leg now
  reports nonzero truthfully; Reviewer must retain the deferral.
- `/usr/bin/true` is only a wrapper propagation mock and supplies no visual QA.
- Historical migrations/rows, external GitHub Projects and Linear views,
  tracker best-effort behavior, and live external state remain protected.
- `ai/STATE.md`, product code, harness scripts, and the Vitest cache were not
  altered by Tester.

## Remaining risks / next action

- `.agentum-harness/qa.sh` is runtime-ignored and untracked. It was available
  and valid for this shared-worktree gate, but its fix is absent from
  `git diff` and will not travel in a commit/fresh checkout. Reviewer should
  inspect this exact runtime artifact and confirm the orchestration path
  preserves or regenerates the truthful contract before relying on another run.
- Review the implementation and evidence. Before release, execute the four
  named scenarios with a real browser verifier and safe fixtures; keep any
  inconclusive result nonzero. Release promotion and live tracker writes remain
  human-gated.
